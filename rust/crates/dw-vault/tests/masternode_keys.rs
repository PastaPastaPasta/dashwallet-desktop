//! Keys attached to tracked masternodes (IOS-082, MG-01 D7) and the seed
//! handed to a provider-key reveal (IOS-083).

#![allow(non_snake_case)] // test names carry checklist ids

mod common;

use common::*;
use dw_vault::{Credential, GrantPurpose, MASTERNODE_KEY_ROLES, UnlockScope, VaultError};

const HASH: [u8; 32] = [0x5a; 32];

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

fn reveal(v: &dw_vault::Vault, credential: Credential<'_>) -> String {
    v.authorize(GrantPurpose::RevealSecret, Some(&wallet(1)), credential)
        .unwrap()
        .id
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
    assert_eq!(v.masternode_key_roles(&HASH), vec!["owner", "voting"]);
    assert!(v.masternode_key_roles(&[0; 32]).is_empty());
    let read = v
        .masternode_key(
            &wallet(1),
            &reveal(&v, Credential::Passphrase(PASS)),
            &HASH,
            "voting",
        )
        .unwrap()
        .unwrap();
    assert_eq!(&read[..], b"voting-wif-2");
    assert!(
        v.masternode_key(
            &wallet(1),
            &reveal(&v, Credential::Passphrase(PASS)),
            &HASH,
            "payout"
        )
        .unwrap()
        .is_none()
    );

    drop(v);
    let v = fx.open();
    assert_eq!(v.masternode_key_roles(&HASH), vec!["owner", "voting"]);
    // Deleting needs the full-scope key.
    assert_eq!(
        v.delete_masternode_keys(&HASH, None).unwrap_err(),
        VaultError::Locked
    );
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(v.delete_masternode_keys(&HASH, Some("payout")).unwrap(), 0);
    assert_eq!(v.delete_masternode_keys(&HASH, Some("owner")).unwrap(), 1);
    assert_eq!(v.masternode_key_roles(&HASH), vec!["voting"]);
    assert_eq!(v.delete_masternode_keys(&HASH, None).unwrap(), 1);
    assert!(v.masternode_key_roles(&HASH).is_empty());
    // The wallet's own records are untouched.
    assert_eq!(v.status().wallets_with_secrets, vec![wallet(1)]);
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
    assert!(matches!(
        v.store_masternode_key(&HASH, "owner", b""),
        Err(VaultError::InvalidArgument(_))
    ));
    assert!(matches!(
        v.store_masternode_key(&HASH, "owner", &[1; dw_vault::MAX_MASTERNODE_KEY_BYTES + 1]),
        Err(VaultError::InvalidArgument(_))
    ));
    assert!(v.masternode_key_roles(&HASH).is_empty());
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
    let grant = reveal(&v, Credential::Passphrase(PASS));
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
    let grant = reveal(&v, Credential::Passphrase(PASS));
    v.lock();
    assert!(
        v.masternode_key(&wallet(1), &grant, &HASH, "owner")
            .is_err()
    );
}
