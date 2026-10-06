//! Keys attached to tracked masternodes (IOS-082) and the seed handed to a
//! provider-key reveal (IOS-083), docs/contracts/m3-engine.md §2.5.

#![allow(non_snake_case)] // test names carry checklist ids

mod common;

use common::*;
use dw_vault::{Credential, GrantKind, GrantPurpose, VaultError};

const HASH: [u8; 32] = [0x5a; 32];

#[test]
fn test_IOS_082_attached_key_round_trips_and_survives_reopen() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();

    let grant = v
        .authorize(
            GrantPurpose::MasternodeOp,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    // A grant bound to any wallet is found without being consumed.
    assert_eq!(v.grant_wallet(&grant.id), Some(wallet(1)));
    assert_eq!(v.grant_wallet("missing"), None);
    let token = v
        .redeem_grant(&grant.id, GrantKind::MasternodeOp, Some(&wallet(1)))
        .unwrap();
    v.store_masternode_key(&token, &HASH, "operator", b"secret-bytes")
        .unwrap();
    assert_eq!(v.masternode_key_roles(&HASH), vec!["operator".to_string()]);
    assert_eq!(
        &v.masternode_key(&token, &HASH, "operator")
            .unwrap()
            .unwrap()[..],
        b"secret-bytes"
    );
    assert!(v.masternode_key(&token, &HASH, "voting").unwrap().is_none());
    assert!(matches!(
        v.store_masternode_key(&token, &HASH, "Bad Role", b"x"),
        Err(VaultError::InvalidArgument(_))
    ));

    drop(v);
    let v = fx.open();
    v.unlock(PASS, dw_vault::UnlockScope::Full).unwrap();
    assert_eq!(v.masternode_key_roles(&HASH), vec!["operator".to_string()]);
    assert_eq!(v.delete_masternode_keys(&HASH, Some("voting")).unwrap(), 0);
    assert_eq!(v.delete_masternode_keys(&HASH, None).unwrap(), 1);
    assert!(v.masternode_key_roles(&HASH).is_empty());
}

#[test]
fn test_IOS_082_attaching_needs_a_masternode_grant() {
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
    let token = v
        .redeem_grant(&grant.id, GrantKind::SignMessage, Some(&wallet(1)))
        .unwrap();
    assert_eq!(
        v.store_masternode_key(&token, &HASH, "owner", b"k"),
        Err(VaultError::GrantPurposeMismatch)
    );
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
