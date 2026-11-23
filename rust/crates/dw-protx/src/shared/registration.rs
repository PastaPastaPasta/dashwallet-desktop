//! The version-3 ProRegTx (`ExtAddr`, rc.3 `evo/providertx.h:162-227`),
//! the form of every registration once v24 is active and the only form a
//! shared one has:
//!
//! ```text
//! u16 nVersion = 3 | u16 nType | u16 nMode | COutPoint collateral | ExtNetInfo
//! | [20] keyIDOwner | [48] pubKeyOperator (basic) | [20] keyIDVoting | u16 nOperatorReward
//! | u8 n, n × MasternodePayoutShare | u8 m, m × CCollateralShare | m × [65] join signature
//! | u32 nEarlyPeriodBlocks | i64 nEarlyPenalty | [32] inputsHash
//! | (Evo) [20] platformNodeID | cs vchSig
//! ```
//!
//! A shared registration has a share table, no payouts, a null owner key, a
//! null collateral hash naming the template output of its own transaction
//! ([`SHARED_COLLATERAL_SCRIPT`]) and an empty `vchSig`; each share owner's
//! consent is a join signature over [`ProRegTxV3::consent_hash`]. Whether a
//! payload is valid is [`crate::rules`]'s question, not the codec's.

use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};
use dashcore::hashes::{Hash, HashEngine, sha256d};
use dashcore::{OutPoint, Txid};

use super::netinfo::ExtNetInfo;
use super::rawtx::SpecialTx;
use super::{CollateralShare, CompactSignature, PayoutShare, io_err, read_u8};

/// `ProTxVersion::ExtAddr` (rc.3 `evo/types.h:17-19`).
pub const PROTX_VERSION_EXT_ADDR: u16 = 3;
/// `TRANSACTION_PROVIDER_REGISTER` (rc.3 `primitives/transaction.h:30`).
pub const TRANSACTION_PROVIDER_REGISTER: u16 = 1;
/// `MnType` (rc.3 `evo/dmn_types.h:13-15`).
pub const MN_TYPE_REGULAR: u16 = 0;
pub const MN_TYPE_EVO: u16 = 1;

/// `SharedCollateralScript()`: `<'DSHC'> OP_DROP OP_TRUE`, the output a
/// shared registration's collateral is (rc.3 `evo/sharedcollateral.h:10-22`);
/// always compared as exact bytes (`:24-27`).
pub const SHARED_COLLATERAL_SCRIPT: [u8; 7] = [0x04, b'D', b'S', b'H', b'C', 0x75, 0x51];

/// A join signature not yet given: 65 zero bytes, as
/// `PrepareSharedRegistration` writes them (rc.3
/// `evo/providertx_service.cpp:1074-1075`).
pub const NULL_JOIN_SIGNATURE: CompactSignature = [0; 65];

/// `CProRegTx` at version 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProRegTxV3 {
    pub mn_type: u16,
    pub mode: u16,
    pub collateral: OutPoint,
    pub net_info: ExtNetInfo,
    pub owner_key_id: [u8; 20],
    /// Basic-scheme serialization; a version-3 payload never has the legacy
    /// one.
    pub operator_public_key: [u8; 48],
    pub voting_key_id: [u8; 20],
    /// Hundredths of a percent.
    pub operator_reward: u16,
    pub payouts: Vec<PayoutShare>,
    pub shares: Vec<CollateralShare>,
    /// One per share, in share order.
    pub join_sigs: Vec<CompactSignature>,
    pub early_period_blocks: u32,
    /// Duffs; `CAmount` (signed) in Core.
    pub early_penalty: i64,
    pub inputs_hash: [u8; 32],
    /// Evo masternodes only.
    pub platform_node_id: Option<[u8; 20]>,
    pub sig: Vec<u8>,
}

impl ProRegTxV3 {
    /// `IsShared()`: a non-empty share table.
    pub fn is_shared(&self) -> bool {
        !self.shares.is_empty()
    }

    /// `MakeSharedRegConsentHash(tx)` (rc.3 `evo/providertx.cpp:335-373`):
    /// the digest each share owner's join signature commits to. It binds
    /// the funding inputs and every sequence, the lock time, every output,
    /// the share table, the exit terms and the registrar configuration, and
    /// not the join signatures, `inputsHash`, the collateral index, the
    /// owner key, the payouts, `vchSig` or any scriptSig.
    pub fn consent_hash(&self, tx: &SpecialTx) -> [u8; 32] {
        let mut engine = sha256d::Hash::engine();
        let en = &mut engine;
        let tag = "DashSharedMNReg";
        VarInt(tag.len() as u64)
            .consensus_encode(en)
            .expect("engines don't error");
        en.input(tag.as_bytes());
        PROTX_VERSION_EXT_ADDR
            .consensus_encode(en)
            .expect("engines don't error");
        tx.version
            .consensus_encode(en)
            .expect("engines don't error");
        tx.tx_type
            .consensus_encode(en)
            .expect("engines don't error");
        tx.lock_time
            .consensus_encode(en)
            .expect("engines don't error");
        en.input(&tx.inputs_hash());
        for input in &tx.input {
            input
                .sequence
                .consensus_encode(en)
                .expect("engines don't error");
        }
        en.input(&tx.outputs_hash());
        self.mn_type
            .consensus_encode(en)
            .expect("engines don't error");
        self.mode.consensus_encode(en).expect("engines don't error");
        self.net_info
            .consensus_encode(en)
            .expect("engines don't error");
        if self.mn_type == MN_TYPE_EVO {
            // rc.3 adds the platform node id (rc.2 had none).
            en.input(&self.platform_node_id.unwrap_or_default());
        }
        en.input(&self.voting_key_id);
        en.input(&self.operator_public_key);
        self.operator_reward
            .consensus_encode(en)
            .expect("engines don't error");
        (self.shares.len() as u8)
            .consensus_encode(en)
            .expect("engines don't error");
        for share in &self.shares {
            share.consensus_encode(en).expect("engines don't error");
        }
        self.early_period_blocks
            .consensus_encode(en)
            .expect("engines don't error");
        self.early_penalty
            .consensus_encode(en)
            .expect("engines don't error");
        sha256d::Hash::from_engine(engine).to_byte_array()
    }

    /// `consentHash` as an envelope and `shared_register_prepare` show it:
    /// `uint256::ToString()`, byte-reversed lower-case hex.
    pub fn display_hash(hash: &[u8; 32]) -> String {
        let mut h = *hash;
        h.reverse();
        hex::encode(h)
    }

    /// The ProTx hash of the registration in `tx`: its txid.
    pub fn pro_tx_hash(tx: &SpecialTx) -> Txid {
        tx.txid()
    }

    /// Decodes a version-3 payload; any other version is refused (the pin
    /// reads versions 1 and 2), as are trailing bytes.
    pub fn from_payload(bytes: &[u8]) -> Result<Self, encode::Error> {
        dashcore::consensus::deserialize(bytes)
    }

    pub fn to_payload(&self) -> Result<Vec<u8>, std::io::Error> {
        let mut out = Vec::new();
        self.consensus_encode(&mut out)?;
        Ok(out)
    }
}

fn count(len: usize, what: &str) -> Result<u8, std::io::Error> {
    u8::try_from(len).map_err(|_| std::io::Error::other(format!("more than 255 {what}")))
}

impl Encodable for ProRegTxV3 {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        // Core throws on a mismatched object rather than write it
        // (`providertx.h:193-197`).
        if self.join_sigs.len() != self.shares.len() {
            return Err(std::io::Error::other("join signature count mismatch"));
        }
        if (self.mn_type == MN_TYPE_EVO) != self.platform_node_id.is_some() {
            return Err(std::io::Error::other(
                "a platform node id is an Evo masternode's",
            ));
        }
        let mut len = PROTX_VERSION_EXT_ADDR.consensus_encode(w)?;
        len += self.mn_type.consensus_encode(w)?;
        len += self.mode.consensus_encode(w)?;
        len += self.collateral.consensus_encode(w)?;
        len += self.net_info.consensus_encode(w)?;
        w.write_all(&self.owner_key_id)?;
        w.write_all(&self.operator_public_key)?;
        w.write_all(&self.voting_key_id)?;
        len += 20 + 48 + 20;
        len += self.operator_reward.consensus_encode(w)?;
        len += count(self.payouts.len(), "payouts")?.consensus_encode(w)?;
        for payout in &self.payouts {
            len += payout.consensus_encode(w)?;
        }
        len += count(self.shares.len(), "shares")?.consensus_encode(w)?;
        for share in &self.shares {
            len += share.consensus_encode(w)?;
        }
        for sig in &self.join_sigs {
            w.write_all(sig)?;
            len += sig.len();
        }
        len += self.early_period_blocks.consensus_encode(w)?;
        len += self.early_penalty.consensus_encode(w)?;
        w.write_all(&self.inputs_hash)?;
        len += 32;
        if let Some(id) = &self.platform_node_id {
            w.write_all(id)?;
            len += 20;
        }
        len += self.sig.consensus_encode(w)?;
        Ok(len)
    }
}

fn read_array<const N: usize, R: std::io::Read + ?Sized>(
    r: &mut R,
) -> Result<[u8; N], encode::Error> {
    let mut b = [0u8; N];
    r.read_exact(&mut b).map_err(io_err)?;
    Ok(b)
}

impl Decodable for ProRegTxV3 {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        if u16::consensus_decode(r)? != PROTX_VERSION_EXT_ADDR {
            return Err(encode::Error::ParseFailed("not a version-3 ProRegTx"));
        }
        let mn_type = u16::consensus_decode(r)?;
        let mode = u16::consensus_decode(r)?;
        let collateral = OutPoint::consensus_decode(r)?;
        let net_info = ExtNetInfo::consensus_decode(r)?;
        let owner_key_id = read_array::<20, _>(r)?;
        let operator_public_key = read_array::<48, _>(r)?;
        let voting_key_id = read_array::<20, _>(r)?;
        let operator_reward = u16::consensus_decode(r)?;
        let payouts = (0..read_u8(r)?)
            .map(|_| PayoutShare::consensus_decode(r))
            .collect::<Result<Vec<_>, _>>()?;
        let share_count = read_u8(r)?;
        let shares = (0..share_count)
            .map(|_| CollateralShare::consensus_decode(r))
            .collect::<Result<Vec<_>, _>>()?;
        let join_sigs = (0..share_count)
            .map(|_| read_array::<65, _>(r))
            .collect::<Result<Vec<_>, _>>()?;
        let early_period_blocks = u32::consensus_decode(r)?;
        let early_penalty = i64::consensus_decode(r)?;
        let inputs_hash = read_array::<32, _>(r)?;
        let platform_node_id = if mn_type == MN_TYPE_EVO {
            Some(read_array::<20, _>(r)?)
        } else {
            None
        };
        let sig = Vec::<u8>::consensus_decode(r)?;
        Ok(Self {
            mn_type,
            mode,
            collateral,
            net_info,
            owner_key_id,
            operator_public_key,
            voting_key_id,
            operator_reward,
            payouts,
            shares,
            join_sigs,
            early_period_blocks,
            early_penalty,
            inputs_hash,
            platform_node_id,
            sig,
        })
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::{ScriptBuf, TxIn, TxOut, Witness};

    fn share(i: u8, amount: u64) -> CollateralShare {
        CollateralShare {
            amount,
            refund_script: ScriptBuf::from_bytes(vec![0x76, 0xa9, 0x14, i]),
            reward_script: ScriptBuf::new(),
            owner_key_id: [i; 20],
        }
    }

    fn shared(mn_type: u16) -> ProRegTxV3 {
        ProRegTxV3 {
            mn_type,
            mode: 0,
            collateral: OutPoint::new(Txid::all_zeros(), 1),
            net_info: ExtNetInfo::core_p2p(&["1.2.3.4:19999".parse().unwrap()]).unwrap(),
            owner_key_id: [0; 20],
            operator_public_key: [0xaa; 48],
            voting_key_id: [0xbb; 20],
            operator_reward: 0,
            payouts: vec![],
            shares: vec![share(1, 60_000_000_000), share(2, 40_000_000_000)],
            join_sigs: vec![NULL_JOIN_SIGNATURE; 2],
            early_period_blocks: 210_240,
            early_penalty: 1_000_000_000,
            inputs_hash: [0x11; 32],
            platform_node_id: (mn_type == MN_TYPE_EVO).then_some([0xcc; 20]),
            sig: vec![],
        }
    }

    fn funding() -> SpecialTx {
        SpecialTx {
            version: 3,
            tx_type: TRANSACTION_PROVIDER_REGISTER,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([7; 32]), 0),
                script_sig: ScriptBuf::new(),
                sequence: u32::MAX,
                witness: Witness::default(),
            }],
            output: vec![
                TxOut {
                    value: 12_345,
                    script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
                },
                TxOut {
                    value: 100_000_000_000,
                    script_pubkey: ScriptBuf::from_bytes(SHARED_COLLATERAL_SCRIPT.to_vec()),
                },
            ],
            lock_time: 0,
            payload: vec![],
        }
    }

    #[test]
    fn test_QT_126_v3_shared_payload_layout() {
        let p = shared(MN_TYPE_REGULAR);
        let bytes = p.to_payload().unwrap();
        assert_eq!(&bytes[..6], &[3, 0, 0, 0, 0, 0]);
        // Null collateral hash, index 1.
        assert_eq!(&bytes[6..38], &[0; 32]);
        assert_eq!(&bytes[38..42], &[1, 0, 0, 0]);
        let after_net = 42 + 13;
        // keyIDOwner null, operator, voting, reward 0, no payouts, 2 shares.
        assert_eq!(&bytes[after_net..after_net + 20], &[0; 20]);
        let counts = after_net + 20 + 48 + 20 + 2;
        assert_eq!(&bytes[counts..counts + 2], &[0, 2]);
        // Two shares (8 + 5 + 1 + 20 each), then 2 × 65 join signatures with
        // no count byte, period, penalty, inputsHash, and an empty vchSig.
        let tail = counts + 2 + 2 * 34;
        assert_eq!(bytes.len(), tail + 2 * 65 + 4 + 8 + 32 + 1);
        assert_eq!(bytes.last(), Some(&0));
        assert_eq!(ProRegTxV3::from_payload(&bytes).unwrap(), p);
    }

    #[test]
    fn test_QT_126_evo_payload_carries_the_platform_node_id() {
        let p = shared(MN_TYPE_EVO);
        let bytes = p.to_payload().unwrap();
        assert_eq!(
            bytes.len(),
            shared(MN_TYPE_REGULAR).to_payload().unwrap().len() + 20
        );
        assert_eq!(ProRegTxV3::from_payload(&bytes).unwrap(), p);
        let mut missing = p.clone();
        missing.platform_node_id = None;
        assert!(missing.to_payload().is_err());
    }

    #[test]
    fn test_QT_126_mismatched_join_signatures_are_not_written() {
        let mut p = shared(MN_TYPE_REGULAR);
        p.join_sigs.pop();
        assert!(p.to_payload().is_err());
    }

    #[test]
    fn test_QT_126_other_versions_and_trailing_bytes_are_refused() {
        let mut bytes = shared(MN_TYPE_REGULAR).to_payload().unwrap();
        bytes.push(0);
        assert!(ProRegTxV3::from_payload(&bytes).is_err());
        bytes.pop();
        bytes[0] = 2;
        assert!(ProRegTxV3::from_payload(&bytes).is_err());
    }

    #[test]
    fn test_QT_126_consent_hash_binds_terms_and_funding_but_not_signatures() {
        let p = shared(MN_TYPE_REGULAR);
        let tx = funding();
        let base = p.consent_hash(&tx);

        // Not bound: join signatures, inputsHash, collateral index, vchSig,
        // scriptSigs.
        let mut q = p.clone();
        q.join_sigs = vec![[9; 65]; 2];
        q.inputs_hash = [0; 32];
        q.collateral.vout = 7;
        assert_eq!(q.consent_hash(&tx), base);
        let mut signed = tx.clone();
        signed.input[0].script_sig = ScriptBuf::from_bytes(vec![1, 2, 3]);
        assert_eq!(p.consent_hash(&signed), base);

        // Bound: each sequence, the lock time, every output, the share
        // table, the terms.
        let mut t = tx.clone();
        t.input[0].sequence = 0xffff_fffe;
        assert_ne!(p.consent_hash(&t), base);
        let mut t = tx.clone();
        t.lock_time = 1;
        assert_ne!(p.consent_hash(&t), base);
        let mut t = tx.clone();
        t.output[0].value += 1;
        assert_ne!(p.consent_hash(&t), base);
        let mut q = p.clone();
        q.shares[1].reward_script = ScriptBuf::from_bytes(vec![0x51]);
        assert_ne!(q.consent_hash(&tx), base);
        let mut q = p.clone();
        q.early_penalty -= 1;
        assert_ne!(q.consent_hash(&tx), base);
        let mut q = p.clone();
        q.operator_public_key[0] ^= 1;
        assert_ne!(q.consent_hash(&tx), base);
    }

    #[test]
    fn test_QT_126_consent_hash_covers_the_evo_platform_node_id() {
        let p = shared(MN_TYPE_EVO);
        let tx = funding();
        let mut q = p.clone();
        q.platform_node_id = Some([0xdd; 20]);
        assert_ne!(q.consent_hash(&tx), p.consent_hash(&tx));
    }

    #[test]
    fn test_QT_126_display_hash_is_byte_reversed() {
        let mut h = [0u8; 32];
        h[0] = 0xab;
        assert!(ProRegTxV3::display_hash(&h).ends_with("ab"));
    }
}
