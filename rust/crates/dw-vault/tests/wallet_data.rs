//! App data sealed to one wallet's seed (gift card credentials and orders
//! in app.sqlite).

mod common;

use common::*;
use dw_vault::{UnlockScope, VaultError};

#[test]
fn sealed_wallet_data_opens_only_for_its_wallet_and_purpose() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    let sealed = v
        .seal_wallet_data(&wallet(1), "giftcards.account", b"fixture-secret")
        .unwrap();
    assert!(!sealed.windows(14).any(|w| w == b"fixture-secret"));
    let opened = v
        .open_wallet_data(&wallet(1), "giftcards.account", &sealed)
        .unwrap();
    assert_eq!(&opened[..], b"fixture-secret");
    assert!(
        v.open_wallet_data(&wallet(2), "giftcards.account", &sealed)
            .is_err()
    );
    assert!(
        v.open_wallet_data(&wallet(1), "giftcards.order", &sealed)
            .is_err()
    );
}

#[test]
fn the_same_seed_in_another_vault_opens_the_data() {
    let first = Fixture::new();
    let a = first.open();
    a.create(None).unwrap();
    a.store_wallet_secret(&wallet(1), &secret(7)).unwrap();
    let sealed = a
        .seal_wallet_data(&wallet(1), "giftcards.account", b"pw")
        .unwrap();
    let second = Fixture::new();
    let b = second.open();
    b.create(Some(OTHER)).unwrap();
    b.store_wallet_secret(&wallet(1), &secret(7)).unwrap();
    let opened = b
        .open_wallet_data(&wallet(1), "giftcards.account", &sealed)
        .unwrap();
    assert_eq!(&opened[..], b"pw");
}

#[test]
fn a_locked_vault_a_watch_only_wallet_or_a_bad_purpose_seals_nothing() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    assert!(v.seal_wallet_data(&wallet(1), "", b"x").is_err());
    assert!(v.seal_wallet_data(&wallet(1), "a/b", b"x").is_err());
    assert_eq!(
        v.seal_wallet_data(&wallet(9), "giftcards.account", b"x")
            .unwrap_err(),
        VaultError::NoSecret
    );
    let sealed = v
        .seal_wallet_data(&wallet(1), "giftcards.account", b"x")
        .unwrap();
    v.lock();
    assert!(
        v.seal_wallet_data(&wallet(1), "giftcards.account", b"x")
            .is_err()
    );
    assert!(
        v.open_wallet_data(&wallet(1), "giftcards.account", &sealed)
            .is_err()
    );
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    assert!(
        v.open_wallet_data(&wallet(1), "giftcards.account", &sealed)
            .is_err()
    );
}
