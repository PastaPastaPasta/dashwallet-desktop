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
//!
//! Its second use is a masternode's vote on a contested DPNS name (MG-07):
//! [`VoteSigner::sign_name_vote`] lays out the whole Platform transition
//! from a [`NameVote`]'s fields and signs only that (MG-07 D5).

use dashcore::OutPoint;
use dashcore::consensus::Encodable;
use dashcore::hashes::{Hash, hash160, sha256d};
use dashcore::secp256k1::{Message, PublicKey, SecretKey, ecdsa};
use dpp::identifier::MasternodeIdentifiers;
use dpp::platform_value::{BinaryData, Identifier, Value};
use dpp::serialization::{PlatformSerializable, Signable};
use dpp::state_transition::StateTransition;
use dpp::state_transition::masternode_vote_transition::MasternodeVoteTransition;
use dpp::state_transition::masternode_vote_transition::v0::MasternodeVoteTransitionV0;
use dpp::system_data_contracts::SystemDataContract;
use dpp::voting::vote_choices::resource_vote_choice::ResourceVoteChoice;
use dpp::voting::vote_polls::VotePoll;
use dpp::voting::vote_polls::contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll;
use dpp::voting::votes::Vote;
use dpp::voting::votes::resource_vote::ResourceVote;
use dpp::voting::votes::resource_vote::v0::ResourceVoteV0;
use key_wallet::Network;

use crate::SignerError;
use crate::paths::voting_key_path;
use crate::signer::{KeyUse, VaultSigner, WalletSigner};
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
        let digest = Message::from_digest(vote.digest(self.network()));
        self.with_secret(key, |secret| compact(digest, secret))
    }

    /// A masternode's vote on a contested DPNS name: the signed
    /// `MasternodeVoteTransition` Platform takes, laid out here from
    /// `vote`'s fields (MG-07 §2, D5). The vault fills in the rest: the DPNS
    /// poll of `vote.label`, the voter identity
    /// (`create_voter_identifier(pro_tx_hash, hash160(key))`) and key id 0,
    /// which Platform gives every voter identity's voting key. It signs dpp's
    /// `signable_bytes()` as an `ECDSA_HASH160` signer does for
    /// `sign_external` (the compact recoverable signature over their double
    /// SHA-256), so the bytes equal rs-sdk's `PutVote` construction
    /// (`MasternodeVoteTransition::try_from_vote_with_signer`, platform
    /// `82fb33aff8`). A label that is not a contested normalized DPNS label
    /// is [`SignerError::PathNotAllowed`], refused before the key is read.
    pub fn sign_name_vote(
        &self,
        key: &VotingKey,
        vote: &NameVote,
    ) -> Result<SignedNameVote, SignerError> {
        if !is_contested_label(&vote.label) {
            return Err(SignerError::PathNotAllowed(format!(
                "a name vote on {:?}, not a contested DPNS label",
                vote.label
            )));
        }
        let (voting_address, signature) = self.with_secret(key, |secret| {
            let public = PublicKey::from_secret_key(secret);
            let address = hash160::Hash::hash(&public.serialize()).to_byte_array();
            let unsigned = vote.transition(address, BinaryData::default());
            let data = unsigned.signable_bytes().map_err(protocol)?;
            let digest = Message::from_digest(sha256d::Hash::hash(&data).to_byte_array());
            Ok::<_, SignerError>((address, compact(digest, secret)))
        })??;
        let signed = vote.transition(voting_address, BinaryData::new(signature.to_vec()));
        let bytes = signed.serialize_to_bytes().map_err(protocol)?;
        let hash = signed.transaction_id().map_err(protocol)?;
        Ok(SignedNameVote {
            bytes,
            hash,
            voter_identity: voter_identity(&vote.pro_tx_hash, &voting_address),
        })
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

/// Core's `SignCompact` of `digest`: the header `27 + recid + 4` (a
/// compressed key), then r and s; deterministic (RFC 6979). Also what
/// `dashcore::signer::sign` makes for a Platform `ECDSA_HASH160` key.
fn compact(digest: Message, secret: &SecretKey) -> [u8; 65] {
    let sig = ecdsa::RecoverableSignature::sign_ecdsa_recoverable(digest, secret);
    let (recid, rs) = sig.serialize_compact();
    let mut out = [0u8; 65];
    out[0] = 27 + recid.to_u8() + 4;
    out[1..].copy_from_slice(&rs);
    out
}

fn protocol(e: dpp::ProtocolError) -> SignerError {
    SignerError::Derivation(format!("name vote transition: {e}"))
}

/// A masternode's choice in a contested name's poll (dpp's
/// `ResourceVoteChoice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameChoice {
    /// For the contender with this identity id.
    TowardsIdentity([u8; 32]),
    Abstain,
    /// Nobody gets the name.
    Lock,
}

/// The fields of one contested-name vote: what
/// [`VoteSigner::sign_name_vote`] signs (MG-07 §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameVote {
    /// The masternode's proTxHash in Platform's order: Core's display
    /// order, the reverse of the transaction's wire bytes (rs-sdk
    /// `PutVote`'s byte-order note).
    pub pro_tx_hash: [u8; 32],
    /// The contested normalized label (`a-hj-km-np-z01-`, 3–19).
    pub label: String,
    pub choice: NameChoice,
    /// The voter identity's next nonce.
    pub nonce: u64,
}

impl NameVote {
    /// DPNS's poll for the label, as rs-sdk's DPNS queries name it.
    fn poll(&self) -> VotePoll {
        VotePoll::ContestedDocumentResourceVotePoll(ContestedDocumentResourceVotePoll {
            contract_id: SystemDataContract::DPNS.id(),
            document_type_name: "domain".into(),
            index_name: "parentNameAndLabel".into(),
            index_values: vec![Value::Text("dash".into()), Value::Text(self.label.clone())],
        })
    }

    /// The transition voting from the key with hash160 `voting_address`,
    /// carrying `signature`. Version 0 is the only one at the pin.
    fn transition(&self, voting_address: [u8; 20], signature: BinaryData) -> StateTransition {
        let resource_vote_choice = match self.choice {
            NameChoice::TowardsIdentity(id) => ResourceVoteChoice::TowardsIdentity(id.into()),
            NameChoice::Abstain => ResourceVoteChoice::Abstain,
            NameChoice::Lock => ResourceVoteChoice::Lock,
        };
        let vote = Vote::ResourceVote(ResourceVote::V0(ResourceVoteV0 {
            vote_poll: self.poll(),
            resource_vote_choice,
        }));
        let transition = MasternodeVoteTransition::V0(MasternodeVoteTransitionV0 {
            pro_tx_hash: self.pro_tx_hash.into(),
            voter_identity_id: voter_identity(&self.pro_tx_hash, &voting_address).into(),
            vote,
            nonce: self.nonce,
            signature_public_key_id: 0,
            signature,
        });
        transition.into()
    }
}

/// A signed contested-name vote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedNameVote {
    /// The serialized state transition, as DAPI takes it.
    pub bytes: Vec<u8>,
    /// Its hash (dpp `transaction_id`: SHA-256 of `bytes`).
    pub hash: [u8; 32],
    /// The voter identity it votes as.
    pub voter_identity: [u8; 32],
}

/// Platform's voter identity of a masternode (Platform order) and the
/// hash160 of its voting key (Drive `create_voter_identity_v0`).
fn voter_identity(pro_tx_hash: &[u8; 32], voting_address: &[u8; 20]) -> [u8; 32] {
    Identifier::create_voter_identifier(pro_tx_hash, voting_address).to_buffer()
}

/// Whether `label` can be the label of a contested DPNS poll: DPNS's
/// `normalizedLabel` pattern (`^[a-hj-km-np-z0-9][a-hj-km-np-z0-9-]{0,61}
/// [a-hj-km-np-z0-9]$`) within its contested index's `^[a-zA-Z01-]{3,19}$`.
pub fn is_contested_label(label: &str) -> bool {
    let allowed = |c: char| {
        (c.is_ascii_lowercase() && !matches!(c, 'i' | 'l' | 'o')) || matches!(c, '0' | '1' | '-')
    };
    (3..=19).contains(&label.len())
        && label.chars().all(allowed)
        && !label.starts_with('-')
        && !label.ends_with('-')
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
