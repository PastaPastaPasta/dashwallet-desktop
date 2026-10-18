//! Keys attached to tracked masternodes (IOS-082, MG-01 D7) and the seed
//! handed to a provider-key reveal (IOS-083).

#![allow(non_snake_case)] // test names carry checklist ids

mod common;

use common::*;
use dw_vault::{
    Credential, GrantPurpose, MASTERNODE_KEY_ROLES, MAX_MASTERNODE_KEY_BYTES, UnlockScope, Vault,
    VaultError,
};

const HASH: [u8; 32] = [0x5a; 32];

/// A passphrase `RevealSecret` grant for wallet 1.
fn reveal(v: &Vault) -> String {
    v.authorize(
        GrantPurpose::RevealSecret,
        Some(&wallet(1)),
        Credential::Passphrase(PASS),
    )
    .unwrap()
    .id
}

#[test]
fn test_IOS_083_revealed_seed_needs_a_reveal_grant() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    let reveal = v
        .authorize(
            GrantPurpose::RevealSecret,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    let first = v
        .with_revealed_seed(&wallet(1), &reveal.id, |seed| seed[0])
        .unwrap();
    assert_eq!(first, secret(1).seed[0]);
    // The grant is consumed.
    assert_eq!(
        v.with_revealed_seed(&wallet(1), &reveal.id, |_| ())
            .unwrap_err(),
        VaultError::GrantInvalid
    );
}

#[test]
fn test_IOS_083_a_grant_of_another_purpose_reveals_nothing() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    let grant = v
        .authorize(
            GrantPurpose::SignMessage,
            Some(&wallet(1)),
            Credential::None,
        )
        .unwrap();
    assert!(v.with_revealed_seed(&wallet(1), &grant.id, |_| ()).is_err());
}

#[test]
fn test_IOS_082_attached_key_round_trips_and_survives_reopen() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();

    v.store_masternode_key(&HASH, "voting", b"voting-wif")
        .unwrap();
    v.store_masternode_key(&HASH, "owner", b"owner-wif")
        .unwrap();
    v.store_masternode_key(&HASH, "voting", b"voting-wif-2")
        .unwrap();
    assert_eq!(v.masternode_key_roles(&HASH).unwrap(), ["owner", "voting"]);
    assert!(v.masternode_key_roles(&[0; 32]).unwrap().is_empty());
    let read = v
        .masternode_key(&wallet(1), &reveal(&v), &HASH, "voting")
        .unwrap()
        .unwrap();
    assert_eq!(&read[..], b"voting-wif-2");
    assert!(
        v.masternode_key(&wallet(1), &reveal(&v), &HASH, "payout")
            .unwrap()
            .is_none()
    );

    drop(v);
    let v = fx.open();
    // Listing and deleting need the full-scope key.
    assert_eq!(
        v.masternode_key_roles(&HASH).unwrap_err(),
        VaultError::Locked
    );
    assert_eq!(
        v.delete_masternode_keys(&HASH, None).unwrap_err(),
        VaultError::Locked
    );
    // A passphrase grant reads on a locked vault.
    let read = v
        .masternode_key(&wallet(1), &reveal(&v), &HASH, "owner")
        .unwrap()
        .unwrap();
    assert_eq!(&read[..], b"owner-wif");
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(v.masternode_key_roles(&HASH).unwrap(), ["owner", "voting"]);
    assert_eq!(v.delete_masternode_keys(&HASH, Some("payout")).unwrap(), 0);
    assert_eq!(v.delete_masternode_keys(&HASH, Some("owner")).unwrap(), 1);
    assert_eq!(v.masternode_key_roles(&HASH).unwrap(), ["voting"]);
    assert_eq!(v.delete_masternode_keys(&HASH, None).unwrap(), 1);
    assert!(v.masternode_key_roles(&HASH).unwrap().is_empty());
    // The wallet's own records are untouched.
    assert_eq!(v.status().wallets_with_secrets, vec![wallet(1)]);
}

#[test]
fn test_IOS_082_the_vault_file_does_not_name_the_masternode() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_masternode_key(&HASH, "owner", b"owner-wif")
        .unwrap();
    let file = String::from_utf8_lossy(&fx.raw()).into_owned();
    assert!(file.contains("mnkey/"));
    let mut display = HASH;
    display.reverse();
    for form in [hex::encode(HASH), hex::encode(display)] {
        assert!(!file.contains(&form), "the file names the proTxHash");
    }
    // Another vault keys its ids differently.
    let other = Fixture::new();
    let w = other.open();
    w.create(Some(PASS)).unwrap();
    w.store_masternode_key(&HASH, "owner", b"owner-wif")
        .unwrap();
    let id = |raw: Vec<u8>| {
        let text = String::from_utf8_lossy(&raw).into_owned();
        let at = text.find("mnkey/").unwrap();
        text[at..at + 6 + 64].to_string()
    };
    assert_ne!(id(fx.raw()), id(other.raw()));
}

#[test]
fn test_IOS_082_operator_and_platform_secrets_are_refused() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    assert_eq!(MASTERNODE_KEY_ROLES, ["owner", "voting", "payout"]);
    for role in ["operator", "platform", "Owner", ""] {
        assert!(
            matches!(
                v.store_masternode_key(&HASH, role, b"k"),
                Err(VaultError::InvalidArgument(_))
            ),
            "{role:?} must be refused"
        );
        assert!(matches!(
            v.delete_masternode_keys(&HASH, Some(role)),
            Err(VaultError::InvalidArgument(_))
        ));
    }
    for key in [&b""[..], &[1; MAX_MASTERNODE_KEY_BYTES + 1][..]] {
        assert!(matches!(
            v.store_masternode_key(&HASH, "owner", key),
            Err(VaultError::InvalidArgument(_))
        ));
    }
    assert!(v.masternode_key_roles(&HASH).unwrap().is_empty());
}

#[test]
fn test_IOS_082_reading_an_attached_key_needs_a_reveal_grant() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_masternode_key(&HASH, "owner", b"owner-wif")
        .unwrap();

    let sign = v
        .authorize(
            GrantPurpose::SignMessage,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    assert_eq!(
        v.masternode_key(&wallet(1), &sign.id, &HASH, "owner")
            .unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    // A grant bound to another wallet is refused too.
    let grant = reveal(&v);
    assert_eq!(
        v.masternode_key(&wallet(2), &grant, &HASH, "owner")
            .unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    // The grant is consumed by a read.
    assert!(
        v.masternode_key(&wallet(1), &grant, &HASH, "owner")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        v.masternode_key(&wallet(1), &grant, &HASH, "owner")
            .unwrap_err(),
        VaultError::GrantInvalid
    );
    // A lock ends an unused grant.
    let grant = reveal(&v);
    v.lock();
    assert!(
        v.masternode_key(&wallet(1), &grant, &HASH, "owner")
            .is_err()
    );
}

/// MG-03 D31: the id of an attached key, hash160 of its compressed public
/// key, read with the full-scope key and nothing secret returned.
#[test]
fn test_IOS_082_an_attached_keys_id_needs_the_full_scope_key() {
    use dashcore::hashes::{Hash, hash160};
    use dashcore::secp256k1::{PublicKey, SecretKey};
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let secret = [7u8; 32];
    v.store_masternode_key(&HASH, "voting", &secret).unwrap();
    v.store_masternode_key(&HASH, "owner", b"not a secret key")
        .unwrap();
    let public = PublicKey::from_secret_key(&SecretKey::from_secret_bytes(secret).unwrap());
    let id = hash160::Hash::hash(&public.serialize()).to_byte_array();
    assert_eq!(v.masternode_key_id(&HASH, "voting").unwrap(), Some(id));
    assert_eq!(v.masternode_key_id(&HASH, "owner").unwrap(), None);
    assert_eq!(v.masternode_key_id(&HASH, "payout").unwrap(), None);
    assert!(matches!(
        v.masternode_key_id(&HASH, "operator").unwrap_err(),
        VaultError::InvalidArgument(_)
    ));
    drop(v);
    let v = fx.open();
    assert_eq!(
        v.masternode_key_id(&HASH, "voting").unwrap_err(),
        VaultError::Locked
    );
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(v.masternode_key_id(&HASH, "voting").unwrap(), Some(id));
}
