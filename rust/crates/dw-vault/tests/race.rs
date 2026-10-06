//! Review H1: concurrent read-modify-writes of the vault file must not lose
//! each other's changes.

mod common;

use std::sync::{Arc, Barrier};
use std::thread;

use common::*;
use dw_vault::{KdfParams, LockState, UnlockScope, VaultError};

/// Argon2id cost that takes a few milliseconds, so a passphrase change spans
/// several record commits on the other thread.
const SLOW_KDF: KdfParams = KdfParams {
    m_kib: 4 * 1024,
    t: 2,
    p: 1,
};

/// `change_passphrase` on one thread while another stores wallet secrets:
/// every stored wallet survives, in memory and after a reopen, and the
/// manifest still verifies (no "vault rolled back").
#[test]
fn change_passphrase_concurrent_with_store_keeps_every_record() {
    const WALLETS: u8 = 24;
    const CHANGES: usize = 12;

    let fx = Fixture::with_kdf(SLOW_KDF);
    let vault = fx.open();
    vault.create(Some(PASS)).unwrap();

    let start = Arc::new(Barrier::new(2));
    let changer = {
        let vault = vault.clone();
        let start = start.clone();
        thread::spawn(move || {
            start.wait();
            for i in 0..CHANGES {
                let (old, new) = if i % 2 == 0 {
                    (PASS, OTHER)
                } else {
                    (OTHER, PASS)
                };
                vault.change_passphrase(old, new).unwrap();
            }
        })
    };
    let storer = {
        let vault = vault.clone();
        let start = start.clone();
        thread::spawn(move || {
            start.wait();
            for n in 1..=WALLETS {
                vault.store_wallet_secret(&wallet(n), &secret(n)).unwrap();
            }
        })
    };
    changer.join().expect("changer thread");
    storer.join().expect("storer thread");

    let expected: Vec<_> = (1..=WALLETS).map(wallet).collect();
    assert_eq!(vault.status().wallets_with_secrets, expected);
    for n in 1..=WALLETS {
        // Reads verify the manifest against the high-water mark.
        vault.seed_derivation(&wallet(n)).unwrap();
    }

    // CHANGES is even, so PASS is current again.
    let reopened = fx.open();
    assert_eq!(reopened.status().wallets_with_secrets, expected);
    reopened
        .unlock(PASS, UnlockScope::Full)
        .expect("unlock after reopen");
    assert_eq!(reopened.lock_state(), LockState::Unlocked);
    for n in 1..=WALLETS {
        reopened.seed_derivation(&wallet(n)).unwrap();
    }
}

/// Review Low: parallel wrong attempts cannot all pass the throttle check
/// before any of them is recorded. With two failures on record the next
/// attempt is the last one allowed; of four parallel ones, exactly one is
/// checked and the other three are throttled.
#[test]
fn parallel_wrong_attempts_respect_the_throttle() {
    const THREADS: usize = 4;

    let fx = Fixture::with_kdf(SLOW_KDF);
    let vault = fx.open();
    vault.create(Some(PASS)).unwrap();
    vault.lock();
    for _ in 0..2 {
        assert!(matches!(
            vault.unlock(b"wrong", UnlockScope::Full),
            Err(VaultError::WrongPassphrase { .. })
        ));
    }

    let start = Arc::new(Barrier::new(THREADS));
    let results: Vec<_> = (0..THREADS)
        .map(|_| {
            let vault = vault.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                vault.unlock(b"wrong", UnlockScope::Full)
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().expect("attempt thread"))
        .collect();

    let wrong = results
        .iter()
        .filter(|r| matches!(r, Err(VaultError::WrongPassphrase { .. })))
        .count();
    let throttled = results
        .iter()
        .filter(|r| matches!(r, Err(VaultError::Throttled { .. })))
        .count();
    assert_eq!((wrong, throttled), (1, THREADS - 1), "{results:?}");
    assert_eq!(vault.status().failed_attempts, 3);
    assert_eq!(fx.open().status().failed_attempts, 3);
}

/// A failed passphrase attempt (which persists the throttle) racing record
/// commits must not drop the records either.
#[test]
fn failed_attempts_concurrent_with_store_keep_every_record() {
    const WALLETS: u8 = 16;

    let fx = Fixture::with_kdf(SLOW_KDF);
    let vault = fx.open();
    vault.create(Some(PASS)).unwrap();

    let start = Arc::new(Barrier::new(2));
    let guesser = {
        let vault = vault.clone();
        let start = start.clone();
        thread::spawn(move || {
            start.wait();
            // Two wrong guesses stay below the throttle; the right one resets it.
            for _ in 0..4 {
                let _ = vault.change_passphrase(b"wrong", OTHER);
                let _ = vault.change_passphrase(b"wrong", OTHER);
                vault.change_passphrase(PASS, PASS).unwrap();
            }
        })
    };
    let storer = {
        let vault = vault.clone();
        thread::spawn(move || {
            start.wait();
            for n in 1..=WALLETS {
                vault.store_wallet_secret(&wallet(n), &secret(n)).unwrap();
            }
        })
    };
    guesser.join().expect("guesser thread");
    storer.join().expect("storer thread");

    let expected: Vec<_> = (1..=WALLETS).map(wallet).collect();
    assert_eq!(vault.status().wallets_with_secrets, expected);
    assert_eq!(fx.open().status().wallets_with_secrets, expected);
}
