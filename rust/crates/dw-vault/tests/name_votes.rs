//! MG-07 (docs/design/MG-07-contested-voting.md §2, D5): the vote signer's
//! contested-name votes, held byte-equal to Platform's own construction
//! (`MasternodeVoteTransition::try_from_vote_with_signer`, which rs-sdk's
//! `PutVote` calls) at the pin.

mod common;

use std::str::FromStr;

use async_trait::async_trait;
use common::*;
use dashcore::hashes::{Hash, hash160, sha256, sha256d};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dpp::address_funds::AddressWitness;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::signer::Signer;
use dpp::identity::{IdentityPublicKey, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::{BinaryData, Identifier, Value};
use dpp::serialization::PlatformSerializable;
use dpp::state_transition::masternode_vote_transition::MasternodeVoteTransition;
use dpp::state_transition::masternode_vote_transition::methods::MasternodeVoteTransitionMethodsV0;
use dpp::system_data_contracts::SystemDataContract;
use dpp::version::PlatformVersion;
use dpp::voting::vote_choices::resource_vote_choice::ResourceVoteChoice;
use dpp::voting::vote_polls::VotePoll;
use dpp::voting::vote_polls::contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll;
use dpp::voting::votes::Vote;
use dpp::voting::votes::resource_vote::ResourceVote;
use dpp::voting::votes::resource_vote::v0::ResourceVoteV0;
use dpp::{ProtocolError, identifier::MasternodeIdentifiers};
use dw_vault::{
    Credential, GrantPurpose, NameChoice, NameVote, SignerError, UnlockScope, Vault, VoteSigner,
    VotingKey, is_contested_label,
};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

const PRO_TX: [u8; 32] = [0x5a; 32];
const ATTACHED: [u8; 32] = [0x07; 32];
const CONTENDER: [u8; 32] = [0xc4; 32];

/// An encrypted vault holding wallet 1, unlocked, with `PRO_TX`'s voting
/// key attached.
fn encrypted(fx: &Fixture) -> Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_masternode_key(&PRO_TX, "voting", &ATTACHED)
        .unwrap();
    v
}

fn vote_signer(v: &Vault) -> VoteSigner {
    let purpose = GrantPurpose::Governance;
    let grant = v
        .authorize(purpose, Some(&wallet(1)), Credential::None)
        .unwrap();
    let token = v
        .redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
        .unwrap();
    v.vote_signer(&wallet(1), &token).unwrap()
}

/// Wallet 1's keychain voting key `index`, derived here.
fn keychain_secret(v: &Vault, index: u32) -> SecretKey {
    let master = ExtendedPrivKey::new_master(v.network(), &[1; 64]).unwrap();
    let path = DerivationPath::from_str(&format!("m/9'/1'/3'/1'/{index}")).unwrap();
    master.derive_priv(&path).unwrap().private_key
}

fn hash160_of(secret: &SecretKey) -> [u8; 20] {
    hash160::Hash::hash(&PublicKey::from_secret_key(secret).serialize()).to_byte_array()
}

/// A single-key `ECDSA_HASH160` signer, as rs-sdk's simple-signer
/// `SingleKeySigner` signs: `dashcore::signer::sign` of the data.
#[derive(Debug)]
struct OneKey(SecretKey);

#[async_trait]
impl Signer<IdentityPublicKey> for OneKey {
    async fn sign(&self, _: &IdentityPublicKey, data: &[u8]) -> Result<BinaryData, ProtocolError> {
        let sig = dpp::dashcore::signer::sign(data, &self.0.to_secret_bytes())
            .map_err(|e| ProtocolError::Generic(e.to_string()))?;
        Ok(sig.to_vec().into())
    }

    async fn sign_create_witness(
        &self,
        _: &IdentityPublicKey,
        _: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        unreachable!("a vote has no address witness")
    }

    fn can_sign_with(&self, _: &IdentityPublicKey) -> bool {
        true
    }
}

/// Platform's construction of `vote`, signed with `secret`: rs-sdk's
/// `PutVote` with the voting key `voter_identity_voting_key` builds.
async fn platform(vote: &NameVote, secret: &SecretKey) -> (Vec<u8>, [u8; 32]) {
    let key = IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id: 0,
        purpose: Purpose::VOTING,
        security_level: SecurityLevel::HIGH,
        contract_bounds: None,
        key_type: KeyType::ECDSA_HASH160,
        read_only: true,
        data: hash160_of(secret).to_vec().into(),
        disabled_at: None,
    });
    let resource_vote_choice = match vote.choice {
        NameChoice::TowardsIdentity(id) => ResourceVoteChoice::TowardsIdentity(id.into()),
        NameChoice::Abstain => ResourceVoteChoice::Abstain,
        NameChoice::Lock => ResourceVoteChoice::Lock,
    };
    let poll = ContestedDocumentResourceVotePoll {
        contract_id: SystemDataContract::DPNS.id(),
        document_type_name: "domain".into(),
        index_name: "parentNameAndLabel".into(),
        index_values: vec![Value::Text("dash".into()), Value::Text(vote.label.clone())],
    };
    let st = MasternodeVoteTransition::try_from_vote_with_signer(
        Vote::ResourceVote(ResourceVote::V0(ResourceVoteV0 {
            vote_poll: VotePoll::ContestedDocumentResourceVotePoll(poll),
            resource_vote_choice,
        })),
        &OneKey(*secret),
        Identifier::from(vote.pro_tx_hash),
        &key,
        vote.nonce,
        PlatformVersion::latest(),
        None,
    )
    .await
    .unwrap();
    (
        st.serialize_to_bytes().unwrap(),
        st.transaction_id().unwrap(),
    )
}

fn name_vote(choice: NameChoice, nonce: u64) -> NameVote {
    NameVote {
        pro_tx_hash: PRO_TX,
        label: "c0ntest-n1me".into(),
        choice,
        nonce,
    }
}

const CHOICES: [NameChoice; 3] = [
    NameChoice::TowardsIdentity(CONTENDER),
    NameChoice::Abstain,
    NameChoice::Lock,
];

#[tokio::test]
async fn name_votes_equal_platforms_construction() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = vote_signer(&v);
    let keys = [
        (VotingKey::Keychain { index: 0 }, keychain_secret(&v, 0)),
        (VotingKey::Keychain { index: 19 }, keychain_secret(&v, 19)),
        (
            VotingKey::Attached {
                pro_tx_hash: PRO_TX,
            },
            SecretKey::from_secret_bytes(ATTACHED).unwrap(),
        ),
    ];
    for (key, secret) in keys {
        for (n, choice) in CHOICES.into_iter().enumerate() {
            let vote = name_vote(choice, 1 + n as u64);
            let signed = s.sign_name_vote(&key, &vote).unwrap();
            let (bytes, hash) = platform(&vote, &secret).await;
            assert_eq!(signed.bytes, bytes, "{key:?} {choice:?}");
            assert_eq!(signed.hash, hash);
            assert_eq!(
                signed.hash,
                sha256::Hash::hash(&signed.bytes).to_byte_array()
            );
            assert_eq!(
                signed.voter_identity,
                Identifier::create_voter_identifier(&PRO_TX, &hash160_of(&secret)).to_buffer()
            );
            // Deterministic (RFC 6979), as Core's `SignCompact`.
            assert_eq!(s.sign_name_vote(&key, &vote).unwrap(), signed);
        }
    }
}

/// The signature is the voting key's compact signature over the double
/// SHA-256 of the transition's signable bytes, with key id 0.
#[test]
fn a_name_vote_recovers_its_voting_key() {
    use dpp::serialization::Signable;
    use dpp::state_transition::StateTransition;

    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = vote_signer(&v);
    let key = VotingKey::Attached {
        pro_tx_hash: PRO_TX,
    };
    let signed = s
        .sign_name_vote(&key, &name_vote(NameChoice::Abstain, 3))
        .unwrap();
    // Decoded as Drive decodes what arrives: exact, in the active version.
    let st = StateTransition::deserialize_from_bytes_untrusted_exact_in_version(
        &signed.bytes,
        PlatformVersion::latest(),
    )
    .unwrap();
    let StateTransition::MasternodeVote(MasternodeVoteTransition::V0(vote)) = &st else {
        panic!("{st:?}");
    };
    assert_eq!(vote.pro_tx_hash.to_buffer(), PRO_TX);
    assert_eq!(vote.voter_identity_id.to_buffer(), signed.voter_identity);
    assert_eq!(vote.nonce, 3);
    assert_eq!(vote.signature_public_key_id, 0);
    let sig = vote.signature.to_vec();
    assert!((31..=34).contains(&sig[0]), "header {}", sig[0]);
    let recid = RecoveryId::try_from(i32::from(sig[0] - 31)).unwrap();
    let rsig = RecoverableSignature::from_compact(&sig[1..], recid).unwrap();
    let digest = sha256d::Hash::hash(&st.signable_bytes().unwrap()).to_byte_array();
    let public = rsig.recover_ecdsa(Message::from_digest(digest)).unwrap();
    assert_eq!(
        hash160::Hash::hash(&public.serialize()).to_byte_array(),
        s.key_id(&key).unwrap()
    );
}

#[test]
fn contested_labels_follow_dpns() {
    for label in ["abc", "a-b", "c0ntest-n1me", "01abcdefghjkmnpqrst", "111"] {
        assert!(is_contested_label(label), "{label}");
    }
    for label in [
        "",
        "ab",
        "01abcdefghjkmnpqrstu", // 20
        "c0ntest-n4me",         // only 0 and 1 of the digits
        "-abc",
        "abc-",
        "alice", // `i` and `l` normalize to `1`
        "bob",   // `o` normalizes to `0`
        "Abc",
        "ab.c",
        "abç",
        "ab c",
    ] {
        assert!(!is_contested_label(label), "{label}");
    }
}

/// A label outside the contested set is refused before any key is read:
/// with no such key, and with the vault locked.
#[test]
fn a_non_contested_label_is_refused_before_the_key() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = vote_signer(&v);
    let no_key = VotingKey::Attached {
        pro_tx_hash: [0x5c; 32],
    };
    let mut vote = name_vote(NameChoice::Lock, 1);
    assert_eq!(s.sign_name_vote(&no_key, &vote), Err(SignerError::NoSecret));
    vote.label = "alice".into();
    assert!(matches!(
        s.sign_name_vote(&no_key, &vote),
        Err(SignerError::PathNotAllowed(_))
    ));
    v.lock();
    assert!(matches!(
        s.sign_name_vote(&VotingKey::Keychain { index: 0 }, &vote),
        Err(SignerError::PathNotAllowed(_))
    ));
}

#[test]
fn a_lock_ends_name_votes() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = vote_signer(&v);
    let keys = [
        VotingKey::Keychain { index: 0 },
        VotingKey::Attached {
            pro_tx_hash: PRO_TX,
        },
    ];
    let vote = name_vote(NameChoice::TowardsIdentity(CONTENDER), 1);
    for key in keys {
        s.sign_name_vote(&key, &vote).unwrap();
    }
    v.lock();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    for key in keys {
        assert_eq!(s.sign_name_vote(&key, &vote), Err(SignerError::Locked));
    }
}
