//! The post-unlock signing window. Unlock already holds the data key; the
//! window is a deadline in this process, not another key and not the
//! passphrase.

mod common;

use common::*;
use std::sync::Arc;

use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantKind, GrantPurpose, LockState,
    SIGNING_WINDOW_SECS, UnlockScope, VaultError,
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
    // The earlier spend is already charged. A new unlock starts the window
    // again, so the zero limit is what the next grant sees.
    v.unlock(PASS, UnlockScope::Full).unwrap();
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
fn two_grants_cannot_add_up_past_the_sealed_limit() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    let half = DEFAULT_QUICK_UNLOCK_SPEND_LIMIT / 2 + 1;
    assert!(v.authorize(spend(half), Some(&w), Credential::None).is_ok());
    assert!(matches!(
        v.authorize(spend(half), Some(&w), Credential::None),
        Err(VaultError::CredentialRequired)
    ));
    assert!(matches!(
        v.authorize_set(&[spend(half), spend(half)], Some(&w), Credential::None,),
        Err(VaultError::CredentialRequired)
    ));
    // The refused set was not charged: what is left still issues.
    let left = DEFAULT_QUICK_UNLOCK_SPEND_LIMIT - half;
    let grant = v
        .authorize(spend(left), Some(&w), Credential::None)
        .unwrap();
    assert!(
        v.window_lease_allowed(std::slice::from_ref(&grant.id))
            .is_ok()
    );
    v.end_signing_window();
    assert!(
        v.check_grant(&grant.id, GrantKind::Spend, Some(&w))
            .is_err()
    );
}

#[test]
fn a_tampered_spend_limit_does_not_raise_the_window() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    let change = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    v.set_quick_unlock_spend_limit(&change.id, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT)
        .unwrap();
    drop(v);
    let mut json = fx.read_json();
    json["quick_unlock"]["spend_limit_duffs"] = serde_json::json!(500_000_000u64);
    fx.write_json(&json);
    let v = fx.open();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 500_000_000);
    assert!(matches!(
        v.authorize(spend(1), Some(&w), Credential::None),
        Err(VaultError::CredentialRequired)
    ));
    assert!(v.signing_window_remaining().is_none());
}

#[test]
fn an_expired_window_does_not_reopen_when_the_wall_clock_moves_back() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    fx.clock.advance(SIGNING_WINDOW_SECS);
    assert!(!v.signing_window_open());
    fx.clock.rewind_wall(SIGNING_WINDOW_SECS + 10_000);
    assert!(!v.signing_window_open());
    assert!(matches!(
        v.authorize(spend(1), Some(&w), Credential::None),
        Err(VaultError::CredentialRequired)
    ));
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(v.signing_window_open());
    assert!(v.authorize(spend(1), Some(&w), Credential::None).is_ok());
}

#[test]
fn one_window_admits_only_one_full_limit_at_a_time() {
    let fx = Fixture::new();
    let v = Arc::new(fx.open());
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    let left = v.clone();
    let right = v.clone();
    let w_left = w;
    let w_right = w;
    let a = std::thread::spawn(move || {
        left.authorize(
            spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT),
            Some(&w_left),
            Credential::None,
        )
    });
    let b = std::thread::spawn(move || {
        right.authorize(
            spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT),
            Some(&w_right),
            Credential::None,
        )
    });
    let results = [a.join().unwrap(), b.join().unwrap()];
    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(ok, 1, "{results:?}");
}

#[test]
fn an_unencrypted_vault_has_no_signing_window() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    assert!(!v.signing_window_open());
    assert!(!v.signing_window_covers(&[spend(1)]));
}

#[test]
fn a_lease_cannot_compose_window_grants_past_the_sealed_limit() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    let first = v
        .authorize(
            spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT),
            Some(&w),
            Credential::None,
        )
        .unwrap();
    // A second unlock starts a new window; the first window's grant is
    // still live.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let platform = v
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: DEFAULT_QUICK_UNLOCK_SPEND_LIMIT * 1000,
            },
            Some(&w),
            Credential::None,
        )
        .unwrap();
    assert!(
        v.window_lease_allowed(std::slice::from_ref(&first.id))
            .is_ok()
    );
    assert!(
        v.window_lease_allowed(std::slice::from_ref(&platform.id))
            .is_ok()
    );
    assert!(matches!(
        v.window_lease_allowed(&[platform.id.clone(), first.id.clone()]),
        Err(VaultError::CredentialRequired)
    ));
    let typed = v
        .authorize(
            spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT),
            Some(&w),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    assert!(v.window_lease_allowed(&[platform.id, typed.id]).is_ok());
}

/// A window grant issued just before the deadline, still inside its TTL
/// after it.
fn grant_at_the_deadline(fx: &Fixture, v: &dw_vault::Vault, credential: Credential<'_>) -> String {
    fx.clock.advance(SIGNING_WINDOW_SECS - 1);
    let grant = v.authorize(spend(1), Some(&wallet(1)), credential).unwrap();
    fx.clock.advance(2);
    grant.id
}

fn unlocked_with_wallet(fx: &Fixture) -> dw_vault::Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

#[test]
fn a_window_grant_ends_with_the_window_though_nothing_asked_about_it() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let id = grant_at_the_deadline(&fx, &v, Credential::None);
    assert!(matches!(
        v.check_grant(&id, GrantKind::Spend, Some(&w)),
        Err(VaultError::GrantInvalid)
    ));
    assert!(matches!(
        v.redeem_grant(&id, GrantKind::Spend, Some(&w)),
        Err(VaultError::GrantInvalid)
    ));
}

#[test]
fn a_lease_check_after_the_deadline_does_not_keep_a_window_grant() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let id = grant_at_the_deadline(&fx, &v, Credential::None);
    assert!(v.window_lease_allowed(std::slice::from_ref(&id)).is_ok());
    assert!(matches!(
        v.redeem_grant(&id, GrantKind::Spend, Some(&w)),
        Err(VaultError::GrantInvalid)
    ));
}

#[test]
fn a_passphrase_grant_outlives_the_window() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let id = grant_at_the_deadline(&fx, &v, Credential::Passphrase(PASS));
    assert!(!v.signing_window_open());
    v.check_grant(&id, GrantKind::Spend, Some(&w)).unwrap();
    assert!(v.window_lease_allowed(std::slice::from_ref(&id)).is_ok());
    v.redeem_grant(&id, GrantKind::Spend, Some(&w)).unwrap();
}

#[test]
fn an_unlock_after_an_unsettled_deadline_does_not_carry_its_grants_over() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let id = grant_at_the_deadline(&fx, &v, Credential::None);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(matches!(
        v.redeem_grant(&id, GrantKind::Spend, Some(&w)),
        Err(VaultError::GrantInvalid)
    ));
}
