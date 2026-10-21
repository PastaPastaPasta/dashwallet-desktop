//! MG-05 (docs/design/MG-05-authorization.md §1–§3, §7 "dw-vault"): the
//! `Governance` and `MasternodeOp` grants, the vote signer and the
//! `MasternodeOwner` signer, with MG-04's owner payload signer (MG-04 D11,
//! D12, D38).

mod common;

use std::str::FromStr;

use common::*;
use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
use dashcore::blockdata::transaction::special_transaction::{
    SpecialTransactionBasePayloadEncodable, TransactionPayload,
};
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dashcore::{OutPoint, Txid};
use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, FundingOutcome, FundingVote, GrantKind,
    GrantPurpose, GrantToken, OwnerKey, SignerError, SignerScope, UnlockScope, Vault, VaultError,
    VoteSigner, VotingKey, WalletSigner,
};
use key_wallet::Signer;
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

const BIP44: &str = "m/44'/1'/0'/0/0";
const GOV: GrantPurpose = GrantPurpose::Governance;
const MN_OP: GrantPurpose = GrantPurpose::MasternodeOp { max_duffs: 1_000 };
const PRO_TX: [u8; 32] = [0x5a; 32];
const ATTACHED: [u8; 32] = [0x07; 32];

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

/// An encrypted vault holding wallet 1, unlocked.
fn encrypted(fx: &Fixture) -> Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

/// A redeemed grant of `purpose` for wallet 1.
fn token(v: &Vault, purpose: GrantPurpose, credential: Credential<'_>) -> GrantToken {
    let grant = v.authorize(purpose, Some(&wallet(1)), credential).unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
        .unwrap()
}

fn hash160_of(secret: &SecretKey) -> [u8; 20] {
    hash160::Hash::hash(&PublicKey::from_secret_key(secret).serialize()).to_byte_array()
}

/// The id of wallet 1's keychain voting key `index`, derived here.
fn keychain_id(v: &Vault, index: u32) -> [u8; 20] {
    let master = ExtendedPrivKey::new_master(v.network(), &[1; 64]).unwrap();
    let key = master
        .derive_priv(&path(&format!("m/9'/1'/3'/1'/{index}")))
        .unwrap();
    hash160_of(&key.private_key)
}

fn vote(time: i64) -> FundingVote {
    FundingVote {
        collateral: OutPoint {
            txid: Txid::from_byte_array([0x11; 32]),
            vout: 1,
        },
        proposal: [0x22; 32],
        outcome: FundingOutcome::Yes,
        time,
    }
}

/// The key id the compact `sig` recovers over `vote`'s digest; it must
/// carry the compressed-key header.
fn signer_of(sig: &[u8; 65], vote: &FundingVote, v: &Vault) -> [u8; 20] {
    assert!((31..=34).contains(&sig[0]), "header {}", sig[0]);
    let recid = RecoveryId::try_from(i32::from(sig[0] - 31)).unwrap();
    let rsig = RecoverableSignature::from_compact(&sig[1..], recid).unwrap();
    let public = rsig
        .recover_ecdsa(Message::from_digest(vote.digest(v.network())))
        .unwrap();
    hash160::Hash::hash(&public.serialize()).to_byte_array()
}

// MARK: The purposes (§1, D2)

#[test]
fn the_purposes_are_wallet_bound_spend_column_with_their_quick_unlock_values() {
    assert_eq!(GOV.kind(), GrantKind::Governance);
    assert_eq!(MN_OP.kind(), GrantKind::MasternodeOp);
    for p in [GOV, MN_OP] {
        assert!(p.wallet_scoped(), "{p:?}");
        assert!(!p.requires_credential(), "{p:?}");
    }
    assert_eq!(GOV.quick_unlock_value(), Some(0));
    assert_eq!(MN_OP.quick_unlock_value(), Some(1_000));

    let fx = Fixture::new();
    let v = encrypted(&fx);
    // Bound to the acting wallet: no grant without one.
    for p in [GOV, MN_OP] {
        assert!(v.authorize(p, None, Credential::None).is_err(), "{p:?}");
    }
    // The `Spend` column: no credential while unlocked with scope Full.
    let t = token(&v, MN_OP, Credential::None);
    assert_eq!(t.max_duffs(), Some(1_000));
    assert_eq!(token(&v, GOV, Credential::None).max_duffs(), None);
    // Locked, the passphrase is asked for.
    v.lock();
    assert_eq!(
        v.authorize(GOV, Some(&wallet(1)), Credential::None)
            .map(drop),
        Err(VaultError::Locked)
    );
    v.authorize(GOV, Some(&wallet(1)), Credential::Passphrase(PASS))
        .unwrap();
}

#[test]
fn quick_unlock_issues_both_within_the_limit() {
    const L: u64 = DEFAULT_QUICK_UNLOCK_SPEND_LIMIT;
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let grant = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    let key = v.enroll_quick_unlock(&grant.id).unwrap();
    v.lock();
    let qu = |purposes: &[GrantPurpose]| {
        v.authorize_set(purposes, Some(&wallet(1)), Credential::QuickUnlock(&key))
            .map(|g| g.len())
    };
    let op = |max_duffs| GrantPurpose::MasternodeOp { max_duffs };
    assert_eq!(qu(&[GOV]), Ok(1));
    assert_eq!(qu(&[op(L)]), Ok(1));
    assert_eq!(qu(&[op(L), GOV]), Ok(2));
    let exceeded = Err(VaultError::QuickUnlockLimitExceeded { limit_duffs: L });
    assert_eq!(qu(&[op(L + 1)]), exceeded);
    assert_eq!(qu(&[op(L), GrantPurpose::Spend { max_duffs: 1 }]), exceeded);
}

// MARK: Which grant issues which signer (§1, §2)

#[test]
fn each_signer_takes_its_own_purpose_only() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let w = wallet(1);
    let mismatch = Err(VaultError::GrantPurposeMismatch);
    let platform = GrantPurpose::PlatformOp {
        max_duffs: 1,
        max_credits: 1,
    };
    let others = [
        GrantPurpose::Spend { max_duffs: 1 },
        GrantPurpose::SignMessage,
        platform,
        GrantPurpose::IdentityScan,
    ];
    for p in others.into_iter().chain([MN_OP]) {
        let t = token(&v, p, Credential::None);
        assert_eq!(v.vote_signer(&w, &t).map(drop), mismatch, "{p:?}");
    }
    for p in others.into_iter().chain([GOV]) {
        let t = token(&v, p, Credential::None);
        assert_eq!(v.masternode_signer(&w, &t).map(drop), mismatch, "{p:?}");
    }
    // Neither grant gets a full signer or a Platform one.
    for p in [GOV, MN_OP] {
        let t = token(&v, p, Credential::None);
        assert_eq!(v.signer(&w, &t).map(drop), mismatch, "{p:?}");
        assert_eq!(
            v.platform_signer(&w, &t, SignerScope::PlatformIdentity)
                .map(drop),
            mismatch,
            "{p:?}"
        );
    }
    // `platform_signer` issues neither scope, whatever the grant.
    let t = token(&v, platform, Credential::None);
    for scope in [SignerScope::MasternodeVoting, SignerScope::MasternodeOwner] {
        assert!(
            matches!(
                v.platform_signer(&w, &t, scope),
                Err(VaultError::InvalidArgument(_))
            ),
            "{scope:?}"
        );
    }
    // Another wallet's grant issues nothing for wallet 1.
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    for p in [GOV, MN_OP] {
        let grant = v.authorize(p, Some(&wallet(2)), Credential::None).unwrap();
        let t = v
            .redeem_grant(&grant.id, p.kind(), Some(&wallet(2)))
            .unwrap();
        assert_eq!(v.vote_signer(&w, &t).map(drop), mismatch);
        assert_eq!(v.masternode_signer(&w, &t).map(drop), mismatch);
    }
    assert_eq!(
        v.masternode_signer(&w, &token(&v, MN_OP, Credential::None))
            .unwrap()
            .scope(),
        SignerScope::MasternodeOwner
    );
}

/// MG-04 D11: the owner signer signs address keys' digests and messages,
/// and no digest at an owner key: those sign owner payloads only.
#[tokio::test]
async fn the_owner_signer_signs_addresses_and_no_owner_digest() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = v
        .masternode_signer(&wallet(1), &token(&v, MN_OP, Credential::None))
        .unwrap();
    s.sign_ecdsa(&path(BIP44), [3; 32]).await.unwrap();
    s.sign_message(&path(BIP44), b"collateral").await.unwrap();
    s.public_key(&path("m/9'/1'/3'/2'/0")).await.unwrap();
    assert!(matches!(
        s.sign_message(&path("m/9'/1'/3'/2'/0"), b"collateral")
            .await,
        Err(SignerError::PathNotAllowed(_))
    ));
    for refused in [
        "m/9'/1'/3'/2'/0",
        "m/9'/1'/3'/1'/0",
        "m/9'/1'/3'/3'/0",
        "m/9'/1'/5'/0'/0'/0'/0'",
    ] {
        assert!(
            matches!(
                s.sign_ecdsa(&path(refused), [3; 32]).await,
                Err(SignerError::PathNotAllowed(_))
            ),
            "{refused}"
        );
    }
}

// MARK: The vote signer (§3, D1, D3)

fn vote_signer(v: &Vault) -> VoteSigner {
    v.vote_signer(&wallet(1), &token(v, GOV, Credential::None))
        .unwrap()
}

#[test]
fn a_keychain_vote_recovers_its_voting_key() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = vote_signer(&v);
    assert_eq!(s.wallet_id(), wallet(1));
    for index in [0, 7, 19] {
        let key = VotingKey::Keychain { index };
        let id = s.key_id(&key).unwrap();
        assert_eq!(id, keychain_id(&v, index), "{index}");
        let vote = vote(1_800_000_000 + i64::from(index));
        let sig = s.sign_vote(&key, &vote).unwrap();
        assert_eq!(signer_of(&sig, &vote, &v), id);
        // Deterministic, as Core's `SignCompact`.
        assert_eq!(s.sign_vote(&key, &vote).unwrap(), sig);
    }
    // A hardened index is no voting key.
    assert!(matches!(
        s.key_id(&VotingKey::Keychain { index: 1 << 31 }),
        Err(SignerError::PathNotAllowed(_))
    ));
}

#[test]
fn an_attached_vote_recovers_the_attached_key() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.store_masternode_key(&PRO_TX, "voting", &ATTACHED)
        .unwrap();
    v.store_masternode_key(&[0x5b; 32], "owner", &[0x08; 32])
        .unwrap();
    let s = vote_signer(&v);
    let key = VotingKey::Attached {
        pro_tx_hash: PRO_TX,
    };
    let id = s.key_id(&key).unwrap();
    assert_eq!(
        id,
        hash160_of(&SecretKey::from_secret_bytes(ATTACHED).unwrap())
    );
    assert_eq!(v.masternode_key_id(&PRO_TX, "voting").unwrap(), Some(id));
    let vote = vote(1_800_000_000);
    assert_eq!(signer_of(&s.sign_vote(&key, &vote).unwrap(), &vote, &v), id);

    // Only a `voting` key: an owner key attached to another masternode, or
    // none at all, is no secret to vote with.
    for pro_tx_hash in [[0x5b; 32], [0x5c; 32]] {
        assert_eq!(
            s.key_id(&VotingKey::Attached { pro_tx_hash }),
            Err(SignerError::NoSecret)
        );
    }
    // A record that is no secp256k1 secret.
    v.store_masternode_key(&[0x5d; 32], "voting", b"not 32 bytes")
        .unwrap();
    assert_eq!(
        s.key_id(&VotingKey::Attached {
            pro_tx_hash: [0x5d; 32]
        }),
        Err(SignerError::NoSecret)
    );
}

#[test]
fn a_lock_ends_the_vote_signer() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.store_masternode_key(&PRO_TX, "voting", &ATTACHED)
        .unwrap();
    let s = vote_signer(&v);
    let keys = [
        VotingKey::Keychain { index: 0 },
        VotingKey::Attached {
            pro_tx_hash: PRO_TX,
        },
    ];
    for key in keys {
        s.sign_vote(&key, &vote(1)).unwrap();
    }
    v.lock();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    for key in keys {
        assert_eq!(s.sign_vote(&key, &vote(1)), Err(SignerError::Locked));
        assert_eq!(s.key_id(&key), Err(SignerError::Locked));
    }
}

/// On a locked vault a passphrase grant carries its own key, so it issues
/// both signers only through a hold (E0-04 §3.5), and dropping the hold
/// stops them.
#[tokio::test]
async fn own_key_grants_sign_through_a_hold_until_it_drops() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.store_masternode_key(&PRO_TX, "voting", &ATTACHED)
        .unwrap();
    v.lock();
    let pass = Credential::Passphrase(PASS);
    let w = wallet(1);
    let mut set = [token(&v, GOV, pass), token(&v, MN_OP, pass)];
    for r in [
        v.vote_signer(&w, &set[0]).map(drop),
        v.masternode_signer(&w, &set[1]).map(drop),
    ] {
        assert!(matches!(r, Err(VaultError::InvalidArgument(_))), "{r:?}");
    }
    let hold = v.hold_key(&mut set).unwrap().unwrap();
    let votes = v.vote_signer_held(&w, &hold, &set[0]).unwrap();
    let owner = v.masternode_signer_held(&w, &hold, &set[1]).unwrap();
    let attached = VotingKey::Attached {
        pro_tx_hash: PRO_TX,
    };
    votes.sign_vote(&attached, &vote(1)).unwrap();
    votes
        .sign_vote(&VotingKey::Keychain { index: 0 }, &vote(1))
        .unwrap();
    owner.sign_ecdsa(&path(BIP44), [3; 32]).await.unwrap();
    drop(hold);
    assert_eq!(
        votes.sign_vote(&attached, &vote(1)),
        Err(SignerError::Locked)
    );
    assert_eq!(
        votes.key_id(&VotingKey::Keychain { index: 0 }),
        Err(SignerError::Locked)
    );
    assert!(matches!(
        owner.sign_ecdsa(&path(BIP44), [3; 32]).await,
        Err(SignerError::Locked)
    ));
}

// MARK: The owner payload signer (MG-04 D11, D12, D38)

fn owner_signer(v: &Vault) -> dw_vault::VaultSigner {
    v.masternode_signer(&wallet(1), &token(v, MN_OP, Credential::None))
        .unwrap()
}

/// A ProUpRegTx payload of `pro_tx_hash` (wire order).
fn update(pro_tx_hash: [u8; 32]) -> ProviderUpdateRegistrarPayload {
    ProviderUpdateRegistrarPayload {
        version: 2,
        pro_tx_hash: Txid::from_byte_array(pro_tx_hash),
        provider_mode: 0,
        operator_public_key: [0x8b; 48].into(),
        voting_key_hash: dashcore::PubkeyHash::from_byte_array([0x33; 20]),
        script_payout: dashcore::ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array(
            [0x44; 20],
        )),
        inputs_hash: dashcore::hash_types::InputsHash::from_byte_array([0x55; 32]),
        payload_sig: Vec::new(),
    }
}

/// The key id `sig` recovers over `payload`'s hash (signature excluded).
fn owner_of(sig: &[u8; 65], payload: &ProviderUpdateRegistrarPayload) -> [u8; 20] {
    assert!((31..=34).contains(&sig[0]), "header {}", sig[0]);
    let recid = RecoveryId::try_from(i32::from(sig[0] - 31)).unwrap();
    let rsig = RecoverableSignature::from_compact(&sig[1..], recid).unwrap();
    let public = rsig
        .recover_ecdsa(Message::from_digest(
            payload.base_payload_hash().to_byte_array(),
        ))
        .unwrap();
    hash160::Hash::hash(&public.serialize()).to_byte_array()
}

#[test]
fn a_keychain_owner_key_signs_the_payload_hash() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let master = ExtendedPrivKey::new_master(v.network(), &[1; 64]).unwrap();
    for index in [0, 19] {
        let key = OwnerKey::Keychain { index };
        let p = update(PRO_TX);
        let sig = s.sign_update_registrar(&key, &p).unwrap();
        let owner = master
            .derive_priv(&path(&format!("m/9'/1'/3'/2'/{index}")))
            .unwrap();
        assert_eq!(
            owner_of(&sig, &p),
            hash160_of(&owner.private_key),
            "{index}"
        );
        // The hash leaves the signature out: a payload already carrying one
        // signs the same.
        let mut signed = p.clone();
        signed.payload_sig = sig.to_vec();
        assert_eq!(s.sign_update_registrar(&key, &signed).unwrap(), sig);
        let mut other = p.clone();
        other.inputs_hash = dashcore::hash_types::InputsHash::from_byte_array([0x56; 32]);
        assert_ne!(s.sign_update_registrar(&key, &other).unwrap(), sig);
    }
    assert!(matches!(
        s.sign_update_registrar(&OwnerKey::Keychain { index: 1 << 31 }, &update(PRO_TX)),
        Err(SignerError::PathNotAllowed(_))
    ));
}

#[test]
fn an_attached_owner_key_signs_its_masternodes_payloads_only() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.store_masternode_key(&PRO_TX, "owner", &ATTACHED).unwrap();
    v.store_masternode_key(&[0x5b; 32], "voting", &[0x08; 32])
        .unwrap();
    let s = owner_signer(&v);
    let key = OwnerKey::Attached {
        pro_tx_hash: PRO_TX,
    };
    let p = update(PRO_TX);
    let sig = s.sign_update_registrar(&key, &p).unwrap();
    assert_eq!(
        owner_of(&sig, &p),
        hash160_of(&SecretKey::from_secret_bytes(ATTACHED).unwrap())
    );
    // Another masternode's payload: refused before the key is read.
    assert!(matches!(
        s.sign_update_registrar(&key, &update([0x5b; 32])),
        Err(SignerError::KeyMismatch(_))
    ));
    // Only an `owner` key: a voting key, or none at all, is no secret.
    let voting_only = OwnerKey::Attached {
        pro_tx_hash: [0x5b; 32],
    };
    assert_eq!(
        s.sign_update_registrar(&voting_only, &update([0x5b; 32])),
        Err(SignerError::NoSecret)
    );
    // A vote signer can't vote with the attached owner key either.
    assert_eq!(
        vote_signer(&v).key_id(&VotingKey::Attached {
            pro_tx_hash: PRO_TX
        }),
        Err(SignerError::NoSecret)
    );
}

/// D38: only a `MasternodeOp` grant's signer makes an owner signature; a
/// spend grant's full signer reaches neither kind of owner key for it.
#[test]
fn only_a_masternode_op_grant_signs_owner_payloads() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.store_masternode_key(&PRO_TX, "owner", &ATTACHED).unwrap();
    let full = v
        .signer(
            &wallet(1),
            &token(&v, GrantPurpose::Spend { max_duffs: 1 }, Credential::None),
        )
        .unwrap();
    assert_eq!(full.scope(), SignerScope::Full);
    for key in [
        OwnerKey::Keychain { index: 0 },
        OwnerKey::Attached {
            pro_tx_hash: PRO_TX,
        },
    ] {
        assert!(
            matches!(
                full.sign_update_registrar(&key, &update(PRO_TX)),
                Err(SignerError::PathNotAllowed(_))
            ),
            "{key:?}"
        );
    }
}

/// dashd's own ProUpRegTx payloads (testdata/protx_update_registrar.json,
/// dashd v24.0.0-rc.3): with dashd's owner key attached, the vault signs
/// each exactly as dashd did (`CKey::SignCompact` is RFC 6979).
#[test]
fn attached_owner_signatures_equal_dashds() {
    let path = format!(
        "{}/../../../testdata/protx_update_registrar.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(doc["source"].as_str().unwrap().contains("v24.0.0-rc.3"));
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let cases = doc["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        let tx: dashcore::Transaction =
            dashcore::consensus::deserialize(&hex::decode(c["tx_hex"].as_str().unwrap()).unwrap())
                .unwrap();
        let Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(p)) =
            tx.special_transaction_payload
        else {
            panic!("not a ProUpRegTx")
        };
        let owner = dashcore::PrivateKey::from_wif(c["owner_wif"].as_str().unwrap()).unwrap();
        let hash = p.pro_tx_hash.to_byte_array();
        v.store_masternode_key(&hash, "owner", &owner.inner.to_secret_bytes())
            .unwrap();
        let sig = s
            .sign_update_registrar(&OwnerKey::Attached { pro_tx_hash: hash }, &p)
            .unwrap();
        assert_eq!(sig.to_vec(), p.payload_sig, "{}", c["kind"]);
    }
}
