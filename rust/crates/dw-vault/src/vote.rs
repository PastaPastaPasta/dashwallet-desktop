//! [`VoteSigner`]: governance votes with the wallet's masternode voting keys
//! (MG-05 §3, D1).
//!
//! A key that signs a 32-byte digest signs whatever that digest is, a
//! transaction's sighash included: coins sent to a voting key's address
//! would be spendable by any digest signer at that path. So a vote signer
//! takes the vote's fields, not a digest. It builds the digest itself, as
//! Dash Core's `CGovernanceVote::Sign` does, and a transaction sighash (the
//! double SHA-256 of another preimage) is never one of them. The signal is
//! fixed to funding: the others are the operator's (MG-02 D89).
//!
//! The digest is a copy of `dw_governance::GovernanceVote::signing_digest`,
//! which this crate cannot depend on (it pulls in the P2P stack); an engine
//! test holds the two equal on every network.

use dashcore::OutPoint;
use dashcore::consensus::Encodable;
use dashcore::hashes::{Hash, hash160, sha256d};
use dashcore::secp256k1::{PublicKey, SecretKey};
use key_wallet::Network;

use crate::SignerError;
use crate::paths::voting_key_path;
use crate::signer::{KeyUse, VaultSigner, WalletSigner, sign_compact};
use crate::types::WalletId;
use crate::vault::masternode::ROLE_VOTING;

/// Core's `VOTE_SIGNAL_FUNDING`.
const SIGNAL_FUNDING: i32 = 1;

/// Which voting key signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VotingKey {
    /// The wallet's keychain voting key `m/9'/coin'/3'/1'/index`.
    Keychain { index: u32 },
    /// The `voting` key attached to the tracked masternode `pro_tx_hash`
    /// (MG-01 D7). Attached keys are vault-wide (MG-05 D3).
    Attached { pro_tx_hash: [u8; 32] },
}

/// A funding vote's outcome (Core `vote_outcome_enum_t`, without `NONE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FundingOutcome {
    Yes,
    No,
    Abstain,
}

impl FundingOutcome {
    /// Core's code: `VOTE_OUTCOME_YES` 1, `NO` 2, `ABSTAIN` 3.
    pub fn code(self) -> i32 {
        match self {
            FundingOutcome::Yes => 1,
            FundingOutcome::No => 2,
            FundingOutcome::Abstain => 3,
        }
    }
}

/// The fields of one funding vote: what [`VoteSigner::sign_vote`] signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FundingVote {
    /// The voting masternode's collateral.
    pub collateral: OutPoint,
    /// The proposal's hash, internal order.
    pub proposal: [u8; 32],
    pub outcome: FundingOutcome,
    /// UNIX seconds.
    pub time: i64,
}

impl FundingVote {
    /// The digest the compact signature commits to on `network`: Core's
    /// `GetSignatureHash()` on testnet (`CHashSigner::SignHash`), else the
    /// `signmessage` hash of `GetSignatureString()`
    /// (`CMessageSigner::SignMessage`).
    pub fn digest(&self, network: Network) -> [u8; 32] {
        if network == Network::Testnet {
            let mut buf = Vec::with_capacity(84);
            self.collateral
                .consensus_encode(&mut buf)
                .expect("writing to a Vec cannot fail");
            buf.extend_from_slice(&self.proposal);
            buf.extend_from_slice(&self.outcome.code().to_le_bytes());
            buf.extend_from_slice(&SIGNAL_FUNDING.to_le_bytes());
            buf.extend_from_slice(&self.time.to_le_bytes());
            sha256d::Hash::hash(&buf).to_byte_array()
        } else {
            let mut proposal = self.proposal;
            proposal.reverse();
            let text = format!(
                "{}-{}|{}|{SIGNAL_FUNDING}|{}|{}",
                self.collateral.txid,
                self.collateral.vout,
                hex::encode(proposal),
                self.outcome.code(),
                self.time
            );
            dw_message::message_hash(text.as_bytes())
        }
    }
}

/// Signs funding votes for one wallet, valid until the vault locks or
/// changes unlock scope, or the hold of its grant's key drops. Obtain it
/// from [`crate::Vault::vote_signer`] under a `Governance` grant. It has no
/// other method: no `Signer`, no message, no public key beyond the key id.
#[derive(Clone)]
pub struct VoteSigner {
    /// Scope [`crate::SignerScope::MasternodeVoting`]: what it reaches, and
    /// its epoch and key.
    signer: VaultSigner,
}

impl std::fmt::Debug for VoteSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoteSigner")
            .field("wallet_id", &hex::encode(self.signer.wallet_id()))
            .finish_non_exhaustive()
    }
}

impl VoteSigner {
    pub(crate) fn new(signer: VaultSigner) -> Self {
        Self { signer }
    }

    /// The wallet whose keychain keys it signs with.
    pub fn wallet_id(&self) -> WalletId {
        self.signer.wallet_id()
    }

    /// The network of the vault (which digest [`Self::sign_vote`] signs).
    pub fn network(&self) -> Network {
        self.signer.network()
    }

    /// The hash160 of `key`'s compressed public key, which a masternode
    /// list entry names as its voting key id. `NoSecret` for an attached
    /// key that is missing or no secp256k1 secret.
    pub fn key_id(&self, key: &VotingKey) -> Result<[u8; 20], SignerError> {
        self.with_secret(key, |secret| {
            let public = PublicKey::from_secret_key(secret);
            hash160::Hash::hash(&public.serialize()).to_byte_array()
        })
    }

    /// The 65-byte compact signature of `vote` with `key`, over the
    /// network's vote digest ([`FundingVote::digest`]); the header is
    /// `27 + recid + 4` (a compressed key), as Core's `SignCompact`, and
    /// signing is deterministic (RFC 6979).
    pub fn sign_vote(&self, key: &VotingKey, vote: &FundingVote) -> Result<[u8; 65], SignerError> {
        let digest = vote.digest(self.network());
        self.with_secret(key, |secret| sign_compact(digest, secret))
    }

    fn with_secret<T>(
        &self,
        key: &VotingKey,
        f: impl FnOnce(&SecretKey) -> T,
    ) -> Result<T, SignerError> {
        match key {
            VotingKey::Keychain { index } => {
                let path = voting_key_path(self.network(), *index);
                self.signer
                    .with_key(&path, KeyUse::Vote, |x| f(&x.private_key))
            }
            VotingKey::Attached { pro_tx_hash } => {
                self.signer
                    .with_attached_key(pro_tx_hash, ROLE_VOTING, KeyUse::Vote, f)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// The two digests of one vote, pinned: Core's layouts (the testnet
    /// preimage and the signature string), recomputed here by hand.
    #[test]
    fn digest_follows_core_layouts() {
        let txid = dashcore::Txid::from_str(
            "0101010101010101010101010101010101010101010101010101010101010102",
        )
        .unwrap();
        let mut proposal = [0u8; 32];
        proposal[0] = 0xaa;
        proposal[31] = 0x0b;
        let vote = FundingVote {
            collateral: OutPoint { txid, vout: 7 },
            proposal,
            outcome: FundingOutcome::No,
            time: 1_700_000_000,
        };
        let text = format!("{txid}-7|0b{}aa|1|2|1700000000", "00".repeat(30));
        for net in [Network::Mainnet, Network::Devnet, Network::Regtest] {
            assert_eq!(vote.digest(net), dw_message::message_hash(text.as_bytes()));
        }
        let mut pre = Vec::new();
        pre.extend_from_slice(&txid.to_byte_array());
        pre.extend_from_slice(&7u32.to_le_bytes());
        pre.extend_from_slice(&proposal);
        pre.extend_from_slice(&2i32.to_le_bytes());
        pre.extend_from_slice(&1i32.to_le_bytes());
        pre.extend_from_slice(&1_700_000_000i64.to_le_bytes());
        assert_eq!(
            vote.digest(Network::Testnet),
            sha256d::Hash::hash(&pre).to_byte_array()
        );
        assert_eq!(
            [
                FundingOutcome::Yes,
                FundingOutcome::No,
                FundingOutcome::Abstain
            ]
            .map(|o| o.code()),
            [1, 2, 3]
        );
    }
}
