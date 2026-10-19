//! `CGovernanceVote` (Dash Core `src/governance/vote.{h,cpp}`): the
//! `govobjvote` serialization, the vote hash, what a voting key signs, and
//! recovering the signer's key id from a compact signature.
//!
//! Signing (Core `CGovernanceVote::CheckSignature`): on testnet the 65-byte
//! compact signature is over the vote's serialization hash
//! (`CHashSigner`); on every other network it is a Dash signed message
//! (`CMessageSigner`, "DarkCoin Signed Message:\n") of
//! `"<txid>-<n>|<parent>|<signal>|<outcome>|<time>"`. BLS (96-byte)
//! signatures are the operator-key form: Core takes only that form for a
//! trigger's votes and for a proposal's `valid`, `delete` and `endorsed`
//! votes (`CGovernanceObject::ProcessVote`'s `onlyVotingKeyAllowed`), over
//! the serialization hash under the basic scheme (MG-02 D89).

use dashcore::Network;
use dashcore::OutPoint;
use dashcore::bls_sig_utils::{BlsPkBytes, BlsScheme, BlsSigBytes};
use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};
use dashcore::hashes::{Hash, hash160, sha256d};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId, Signature};
use dashcore::secp256k1::{Message, PublicKey};

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

/// `VOTE_SIGNAL_FUNDING`: the only signal the wallet casts (dash-qt), and
/// the one budgets and statuses count.
pub const SIGNAL_FUNDING: i32 = 1;

/// The signals besides funding (`vote_signal_enum_t`), tallied for Core's
/// sentinel flags and deletion by vote (MG-02 D82–D84).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SentinelSignal {
    Valid,
    Delete,
    Endorsed,
}

impl SentinelSignal {
    pub const ALL: [Self; 3] = [Self::Valid, Self::Delete, Self::Endorsed];

    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            2 => Some(Self::Valid),
            3 => Some(Self::Delete),
            4 => Some(Self::Endorsed),
            _ => None,
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Valid => 2,
            Self::Delete => 3,
            Self::Endorsed => 4,
        }
    }
}

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

    /// Whether the 96-byte signature is the operator's, of `operator_key`
    /// as the masternode list carries it (Core `CheckSignature(const
    /// CBLSPublicKey&)`: the signature read and checked under the basic
    /// scheme, over [`Self::signature_hash`]). The list writes a version-1
    /// entry's key in the legacy encoding and carries no version here, so a
    /// key that does not verify in the basic encoding is read as legacy.
    /// Legacy bytes read as basic are the key's negation, under which the
    /// negated signature verifies: a copy only the operator's own signature
    /// yields, of the same vote (the hash leaves the signature out).
    pub fn verify_operator(&self, operator_key: &[u8; 48]) -> bool {
        let Ok(sig) = <[u8; BLS_SIG_SIZE]>::try_from(self.signature.as_slice()) else {
            return false;
        };
        let sig = BlsSigBytes::from_bytes(sig);
        let digest = self.signature_hash();
        let key = BlsPkBytes::from_bytes(*operator_key);
        let verifies = |k: BlsPkBytes| k.as_scheme(BlsScheme::Modern).verify(&digest, &sig).is_ok();
        verifies(key)
            || key
                .as_scheme(BlsScheme::Legacy)
                .reencode(BlsScheme::Modern)
                .is_ok_and(verifies)
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
            return Err(encode::Error::ParseFailed(
                "bad governance vote signature size",
            ));
        }
        if r.len() != len {
            return Err(encode::Error::ParseFailed(
                "governance vote length mismatch",
            ));
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
/// `digest`, compressed or not as the header byte says (Core
/// `CPubKey::RecoverCompact`, which reads the recovery id and the
/// compressed flag from `header - 27` with `int` arithmetic and no range
/// check).
pub fn recover_compact(sig: &[u8], digest: &[u8; 32]) -> Option<Vec<u8>> {
    if sig.len() != COMPACT_SIG_SIZE {
        return None;
    }
    let h = i32::from(sig[0]) - 27;
    let recid = RecoveryId::try_from(h & 3).ok()?;
    let compressed = h & 4 != 0;
    let rsig = RecoverableSignature::from_compact(&sig[1..], recid).ok()?;
    let pk = rsig.recover_ecdsa(Message::from_digest(*digest)).ok()?;
    Some(if compressed {
        pk.serialize().to_vec()
    } else {
        pk.serialize_uncompressed().to_vec()
    })
}

/// The one form of the 65-byte compact `sig` among those that recover the
/// same key over any digest (D63): [`recover_compact`] reads only the low 3
/// bits of `header - 27`, and (r, n - s) with the other parity recovers
/// what (r, s) does. Header `27 + flags`, low s. `None` when `sig` is no
/// compact signature, which then recovers nothing.
pub fn canonical_compact(sig: &[u8]) -> Option<[u8; COMPACT_SIG_SIZE]> {
    if sig.len() != COMPACT_SIG_SIZE {
        return None;
    }
    let mut flags = ((i32::from(sig[0]) - 27) & 7) as u8;
    let mut s = Signature::from_compact(&sig[1..]).ok()?;
    let raw = s.serialize_compact();
    s.normalize_s();
    let low = s.serialize_compact();
    if low != raw {
        flags ^= 1;
    }
    let mut out = [0; COMPACT_SIG_SIZE];
    out[0] = 27 + flags;
    out[1..].copy_from_slice(&low);
    Some(out)
}

/// Turns a 64-byte compact ECDSA signature by `pubkey` over `digest` into
/// Core's 65-byte recoverable form (header `31 + recid`, compressed key):
/// the recovery id is the one that recovers `pubkey`.
pub fn to_recoverable_compact(
    compact64: &[u8; 64],
    pubkey: &PublicKey,
    digest: &[u8; 32],
) -> Option<[u8; COMPACT_SIG_SIZE]> {
    for id in 0..4 {
        let recid = RecoveryId::try_from(id).ok()?;
        let Ok(rsig) = RecoverableSignature::from_compact(compact64, recid) else {
            continue;
        };
        if rsig
            .recover_ecdsa(Message::from_digest(*digest))
            .ok()
            .as_ref()
            == Some(pubkey)
        {
            let mut out = [0u8; COMPACT_SIG_SIZE];
            out[0] = 31 + recid.to_u8();
            out[1..].copy_from_slice(compact64);
            return Some(out);
        }
    }
    None
}

/// The vote as Core v24's `gobject getcurrentvotes` prints it
/// (`CGovernanceVote::ToString(tip_mn_list)`:
/// `outpoint:time:outcome:signal:weight`). `weight` is the voting weight of
/// the masternode type whose collateral is `masternode_outpoint` in the tip
/// list, 0 when no masternode has that collateral.
pub fn current_vote_string(vote: &GovernanceVote, weight: u32) -> String {
    // `ConvertOutcomeToString`: 0 is "none", an unknown code "error";
    // `ConvertSignalToString`: 0 and unknown codes are "none".
    let outcome = match vote.outcome {
        0 => "none",
        code => VoteOutcome::from_code(code).map_or("error", VoteOutcome::as_str),
    };
    let signal = match vote.signal {
        1 => "funding",
        2 => "valid",
        3 => "delete",
        4 => "endorsed",
        _ => "none",
    };
    format!(
        "{}:{}:{}:{}:{}",
        outpoint_short(&vote.masternode_outpoint),
        vote.time,
        outcome,
        signal,
        weight
    )
}

/// Every signature that recovers what `v`'s does over any digest:
/// `recover_compact` reads only the low 3 bits of the header byte (32
/// header values each), and (r, n - s) recovers the same key with the other
/// parity bit. 64 in all, `v`'s among them (validator r12).
#[cfg(test)]
pub(crate) fn malleations(v: &GovernanceVote) -> Vec<GovernanceVote> {
    use dashcore::secp256k1::SecretKey;
    let sig = &v.signature;
    assert_eq!(sig.len(), COMPACT_SIG_SIZE);
    let flags = (i32::from(sig[0]) - 27) & 7;
    let s: [u8; 32] = sig[33..].try_into().unwrap();
    // n - s, as the negation of s taken as a scalar.
    let negated = SecretKey::from_secret_bytes(s).unwrap().negate();
    let mut out = Vec::new();
    for (want, s) in [(flags, s), (flags ^ 1, negated.to_secret_bytes())] {
        for b in 0u8..=255 {
            if (i32::from(b) - 27) & 7 == want {
                let mut c = v.clone();
                c.signature = [&[b], &sig[1..33], &s[..]].concat();
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "vote_canonical_tests.rs"]
mod canonical_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::secp256k1::SecretKey;

    #[test]
    fn current_vote_string_prints_unknown_codes_as_core_does() {
        let mut v = sample();
        let suffix = |v: &GovernanceVote| {
            let s = current_vote_string(v, 4);
            s[s.find(':').unwrap()..].to_string()
        };
        assert_eq!(suffix(&v), ":1700000000:yes:funding:4");
        (v.outcome, v.signal) = (0, 0);
        assert_eq!(suffix(&v), ":1700000000:none:none:4");
        (v.outcome, v.signal) = (9, 9);
        assert_eq!(suffix(&v), ":1700000000:error:none:4");
    }

    fn sample() -> GovernanceVote {
        let txid = "f3d0bbd2e2c0e21a6a3a44a0f4b4c2fb4a2a6f1d1b4b9f6cfc2e3e2d1c0b0a09"
            .parse()
            .unwrap();
        GovernanceVote::funding(
            OutPoint::new(txid, 1),
            [3; 32],
            VoteOutcome::Yes,
            1_700_000_000,
        )
    }

    fn sign(vote: &mut GovernanceVote, network: Network, sk: &SecretKey) {
        let digest = vote.signing_digest(network);
        let sig = dashcore::secp256k1::ecdsa::sign(Message::from_digest(digest), sk);
        let pk = PublicKey::from_secret_key(sk);
        let full = to_recoverable_compact(&sig.serialize_compact(), &pk, &digest).unwrap();
        vote.signature = full.to_vec();
    }

    #[test]
    fn signatures_recovering_one_key_have_one_canonical_form() {
        // D63: 64 signatures recover the same key; their canonical form is
        // one. The compressed flag names another key, so it stays.
        let sk = SecretKey::from_secret_bytes([9; 32]).unwrap();
        let mut v = sample();
        sign(&mut v, Network::Regtest, &sk);
        let id = v.signer_key_id(Network::Regtest);
        let canonical = canonical_compact(&v.signature).unwrap();
        let copies = malleations(&v);
        assert_eq!(copies.len(), 64);
        for c in &copies {
            assert_eq!(c.signer_key_id(Network::Regtest), id);
            assert_eq!(canonical_compact(&c.signature), Some(canonical));
        }
        let mut other = v.clone();
        other.signature[0] ^= 4;
        assert_ne!(other.signer_key_id(Network::Regtest), id);
        assert_ne!(canonical_compact(&other.signature), Some(canonical));
        assert_eq!(canonical_compact(&[0; 96]), None);
    }

    #[test]
    fn signer_key_id_recovers_the_voting_key_on_each_network() {
        let sk = SecretKey::from_secret_bytes([9; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&sk);
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
        assert!(
            s.starts_with("f3d0bbd2e2c0e21a6a3a44a0f4b4c2fb4a2a6f1d1b4b9f6cfc2e3e2d1c0b0a09-1|")
        );
        assert!(s.ends_with("|1|1|1700000000"));
    }

    #[test]
    fn verify_operator_checks_a_basic_bls_signature_under_either_key_encoding() {
        // D89 vector: a delete NO signed, outside this repo (MG-01: the
        // wallet never signs with an operator key), by dash-pkc's BLS with
        // the throwaway secret [0x42; 32]; the key in Core's basic encoding
        // and in the legacy one a v1 masternode entry carries.
        use dashcore::Txid;
        use dashcore::hashes::Hash;
        let mut v = GovernanceVote::funding(
            OutPoint::new(Txid::from_byte_array([0x11; 32]), 1),
            [0x22; 32],
            VoteOutcome::No,
            1_700_000_000,
        );
        v.signal = SentinelSignal::Delete.code();
        assert_eq!(
            hex::encode(v.signature_hash()),
            "ee2c57eed913bb2ff83ab5bfef44c01a479334d2b210b8b2796dacc43c7a5a50"
        );
        v.signature = hex::decode(
            "a4b805dfe336a076d09676d361dc682fcecd22dfe2c9b7f6464c563a3f662dc1\
             29b543816609a355d8e016430f831f0805efb986f77f71357127db338df16c13\
             062cb62262cbea463b4dd8908c737ff94565696e5dbb2f7e047920cdf6a15ef0",
        )
        .unwrap();
        let key = |s: &str| <[u8; 48]>::try_from(hex::decode(s).unwrap()).unwrap();
        let basic = key(
            "b5b99c967e4c69822f427db1f6871dd119afb95ab9646ba2e707990a3db31777a59b66f69e89c2055699b0ade7357eae",
        );
        let legacy = key(
            "95b99c967e4c69822f427db1f6871dd119afb95ab9646ba2e707990a3db31777a59b66f69e89c2055699b0ade7357eae",
        );
        assert!(v.verify_operator(&basic));
        assert!(v.verify_operator(&legacy));

        // Any signed field, the signature, its length or the key changed: no.
        let mut other = v.clone();
        other.outcome = VoteOutcome::Yes.code();
        assert!(!other.verify_operator(&basic));
        let mut other = v.clone();
        other.signal = SentinelSignal::Endorsed.code();
        assert!(!other.verify_operator(&basic));
        let mut other = v.clone();
        other.time += 1;
        assert!(!other.verify_operator(&basic));
        let mut other = v.clone();
        other.signature[95] ^= 1;
        assert!(!other.verify_operator(&basic));
        let mut other = v.clone();
        other.signature.pop();
        assert!(!other.verify_operator(&basic));
        let mut wrong = basic;
        wrong[47] ^= 1;
        assert!(!v.verify_operator(&wrong));
        assert!(!v.verify_operator(&[0; 48]));
    }
}
