//! MG-05 (docs/design/MG-05-authorization.md §1–§3, §7 "dw-vault"): the
//! `Governance` and `MasternodeOp` grants, the vote signer and the
//! `MasternodeOwner` signer.

mod common;

use std::str::FromStr;

use common::*;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dashcore::{OutPoint, Txid};
use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, FundingOutcome, FundingVote, GrantKind,
    GrantPurpose, GrantToken, SignerError, SignerScope, UnlockScope, Vault, VaultError, VoteSigner,
    VotingKey, WalletSigner,
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

#[tokio::test]
async fn the_owner_signer_signs_addresses_and_owner_keys_only() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = v
        .masternode_signer(&wallet(1), &token(&v, MN_OP, Credential::None))
        .unwrap();
    s.sign_ecdsa(&path(BIP44), [3; 32]).await.unwrap();
    s.sign_ecdsa(&path("m/9'/1'/3'/2'/0"), [3; 32])
        .await
        .unwrap();
    s.sign_message(&path(BIP44), b"collateral").await.unwrap();
    for refused in [
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
