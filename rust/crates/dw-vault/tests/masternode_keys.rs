//! The seed handed to a provider-key reveal (IOS-083),
//! docs/contracts/m3-engine.md §2.5.

#![allow(non_snake_case)] // test names carry checklist ids

mod common;

use common::*;
use dw_vault::{Credential, GrantPurpose, VaultError};

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
