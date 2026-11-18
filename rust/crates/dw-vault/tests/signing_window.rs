//! The post-unlock signing window. Unlock already holds the data key; the
//! window is a deadline in this process, not another key and not the
//! passphrase.

mod common;

use common::*;
use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantPurpose, LockState, SIGNING_WINDOW_SECS,
    UnlockScope, VaultError,
};

fn spend(max_duffs: u64) -> GrantPurpose {
    GrantPurpose::Spend { max_duffs }
}

#[test]
fn a_passphrase_unlock_covers_the_spend_limit_for_five_minutes() {
    let fx = Fixture::new();
    let v = fx.open();
    assert!(!v.signing_window_open());
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    assert!(v.signing_window_open());
    assert!(v.signing_window_covers(&[spend(1)]));
    assert!(v.signing_window_covers(&[spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT)]));
    assert!(!v.signing_window_covers(&[spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT + 1)]));
    assert!(!v.signing_window_covers(&[spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT), spend(1),]));
    // Reveal, a credential change and a wipe are never covered.
    assert!(!v.signing_window_covers(&[GrantPurpose::RevealSecret]));
    assert!(!v.signing_window_covers(&[GrantPurpose::ChangeCredential]));
    assert!(!v.signing_window_covers(&[GrantPurpose::Wipe]));
    assert!(matches!(
        v.authorize(GrantPurpose::RevealSecret, Some(&w), Credential::None),
        Err(VaultError::CredentialRequired)
    ));
    assert!(v.authorize(spend(1), Some(&w), Credential::None).is_ok());

    let change = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    v.set_quick_unlock_spend_limit(&change.id, 0).unwrap();
    assert!(!v.signing_window_covers(&[spend(1)]));
    assert!(v.signing_window_covers(&[GrantPurpose::Governance]));

    v.lock();
    assert_eq!(v.status().state, LockState::Locked);
    assert!(!v.signing_window_open());

    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(v.signing_window_open());
    fx.clock.advance(SIGNING_WINDOW_SECS - 1);
    assert!(v.signing_window_open());
    fx.clock.advance(1);
    assert!(!v.signing_window_open());

    v.unlock(PASS, UnlockScope::Full).unwrap();
    v.end_signing_window();
    assert_eq!(v.status().state, LockState::Unlocked);
    assert!(!v.signing_window_open());

    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    assert!(!v.signing_window_open());
    assert_eq!(v.status().state, LockState::UnlockedMixingOnly);

    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(v.signing_window_open());
    drop(v);
    let again = fx.open();
    assert_eq!(again.status().state, LockState::Locked);
    assert!(!again.signing_window_open());
}

#[test]
fn an_unencrypted_vault_has_no_signing_window() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    assert!(!v.signing_window_open());
    assert!(!v.signing_window_covers(&[spend(1)]));
}
