//! The post-unlock signing window. Unlock already holds the data key; the
//! window is a deadline in this process, not another key and not the
//! passphrase.

mod common;

use common::*;
use std::str::FromStr;
use std::sync::Arc;

use key_wallet::bip32::DerivationPath;

use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantKind, GrantPurpose, LockState,
    SIGNING_WINDOW_SECS, SignerScope, UnlockScope, VaultError, WalletSigner,
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
    allow_window(&v);
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
    assert!(!v.signing_window_covers(&[GrantPurpose::Governance]));

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
    allow_window(&again);
    assert!(!again.signing_window_open());
}

/// The window is for payments. A vote and a masternode owner operation
/// (a ProUpRegTx costs a fee but can move every future reward) take the
/// passphrase inside it, whatever the limit.
#[test]
fn a_vote_or_a_masternode_operation_is_never_in_the_window() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    assert!(v.signing_window_open());
    for purpose in [
        GrantPurpose::Governance,
        GrantPurpose::MasternodeOp { max_duffs: 0 },
        GrantPurpose::MasternodeOp { max_duffs: 1_000 },
    ] {
        assert!(!v.signing_window_covers(&[purpose]), "{purpose:?}");
        assert_eq!(
            v.authorize(purpose, Some(&w), Credential::None).map(drop),
            Err(VaultError::CredentialRequired),
            "{purpose:?}"
        );
        assert_eq!(
            v.authorize_set(&[spend(1), purpose], Some(&w), Credential::None)
                .map(drop),
            Err(VaultError::CredentialRequired),
            "{purpose:?}"
        );
        v.authorize(purpose, Some(&w), Credential::Passphrase(PASS))
            .unwrap();
    }
    assert_eq!(
        v.signing_window_remaining(),
        Some(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT)
    );
}

/// A limit of 0 makes nothing credential-free, a zero-value payment
/// included.
#[test]
fn a_zero_limit_admits_nothing() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let change = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    v.set_quick_unlock_spend_limit(&change.id, 0).unwrap();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(v.signing_window_open());
    for purpose in [
        spend(0),
        GrantPurpose::PlatformOp {
            max_duffs: 0,
            max_credits: 0,
        },
    ] {
        assert!(!v.signing_window_covers(&[purpose]), "{purpose:?}");
        assert_eq!(
            v.authorize(purpose, Some(&w), Credential::None).map(drop),
            Err(VaultError::CredentialRequired),
            "{purpose:?}"
        );
    }
    v.authorize(spend(0), Some(&w), Credential::Passphrase(PASS))
        .unwrap();
}

#[test]
fn two_grants_cannot_add_up_past_the_sealed_limit() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    allow_window(&v);
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
    allow_window(&v);
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
    allow_window(&v);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    // The tampered plain copy is not shown as the limit: it can't be read,
    // and the limit in force is 0 (#173 item 10, Q17 F1).
    let policy = v.quick_unlock_policy();
    assert!(policy.limit_unreadable);
    assert_eq!(policy.spend_limit_duffs, 0);
    assert!(matches!(
        v.authorize(spend(1), Some(&w), Credential::None),
        Err(VaultError::CredentialRequired)
    ));
    assert!(v.signing_window_remaining().is_none());
}

/// #173 item 10: a sealed spending limit that no longer authenticates (here
/// its plain copy was changed) counts as none, and the setter replaces it
/// after the passphrase check its grant made, instead of failing on the old
/// one. A manifest that does not authenticate is still refused.
#[test]
fn a_damaged_spend_limit_can_be_set_again() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    allow_window(&v);
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    let change = |v: &dw_vault::Vault| {
        v.authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap()
        .id
    };
    v.set_quick_unlock_spend_limit(&change(&v), DEFAULT_QUICK_UNLOCK_SPEND_LIMIT)
        .unwrap();
    drop(v);
    let mut json = fx.read_json();
    json["quick_unlock"]["spend_limit_duffs"] = serde_json::json!(500_000_000u64);
    fx.write_json(&json);
    let v = fx.open();
    allow_window(&v);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(matches!(
        v.authorize(spend(1), Some(&w), Credential::None),
        Err(VaultError::CredentialRequired)
    ));

    let limit = v
        .set_quick_unlock_spend_limit(&change(&v), DEFAULT_QUICK_UNLOCK_SPEND_LIMIT)
        .unwrap();
    assert_eq!(limit.spend_limit_duffs, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT);
    // The new limit is sealed: after a passphrase unlock the window pays up to it.
    v.lock();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(v.authorize(spend(1), Some(&w), Credential::None).is_ok());
    drop(v);
    let v = fx.open();
    allow_window(&v);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        v.quick_unlock_policy().spend_limit_duffs,
        DEFAULT_QUICK_UNLOCK_SPEND_LIMIT
    );
    assert!(v.authorize(spend(1), Some(&w), Credential::None).is_ok());

    // A damaged manifest is not a damaged limit: the setter still refuses.
    drop(v);
    let mut json = fx.read_json();
    let ct = json["manifest"]["ct"].as_str().unwrap().to_owned();
    let flipped = format!(
        "{}{}",
        if ct.starts_with('0') { '1' } else { '0' },
        &ct[1..]
    );
    json["manifest"]["ct"] = serde_json::json!(flipped);
    fx.write_json(&json);
    let v = fx.open();
    let refused = v.unlock(PASS, UnlockScope::Full).is_err()
        || v.authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .and_then(|g| v.set_quick_unlock_spend_limit(&g.id, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT))
        .is_err();
    assert!(refused);
}

#[test]
fn an_expired_window_does_not_reopen_when_the_wall_clock_moves_back() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    allow_window(&v);
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
    allow_window(&v);
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
    allow_window(&v);
    assert!(!v.signing_window_open());
    assert!(!v.signing_window_covers(&[spend(1)]));
}

/// Two window grants that fit the limit when issued no longer fit it once
/// the limit is lowered: a lease cannot redeem them together. A passphrase
/// grant is not counted.
#[test]
fn a_lease_cannot_compose_window_grants_past_the_sealed_limit() {
    const PART: u64 = 6_000_000;
    const LOWER: u64 = 10_000_000;
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let first = v
        .authorize(spend(PART), Some(&w), Credential::None)
        .unwrap();
    let platform = v
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: PART * 1000,
            },
            Some(&w),
            Credential::None,
        )
        .unwrap();
    let change = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    v.set_quick_unlock_spend_limit(&change.id, LOWER).unwrap();
    assert!(!v.signing_window_open());
    for ids in [
        vec![first.id.clone()],
        vec![platform.id.clone()],
        vec![platform.id, first.id],
    ] {
        assert_eq!(
            v.window_lease_allowed(&ids),
            Err(VaultError::GrantInvalid),
            "{ids:?}"
        );
    }
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let platform = v
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: PART * 1000,
            },
            Some(&w),
            Credential::None,
        )
        .unwrap();
    let typed = v
        .authorize(spend(PART), Some(&w), Credential::Passphrase(PASS))
        .unwrap();
    assert!(v.window_lease_allowed(&[platform.id, typed.id]).is_ok());
}

fn set_limit(v: &dw_vault::Vault, limit: u64) {
    let change = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    v.set_quick_unlock_spend_limit(&change.id, limit).unwrap();
}

/// Lowering the spending limit ends the signing window: a grant issued
/// under the old limit, redeemed or not, issues no signer on any payment
/// path and goes into no lease, and the next payment asks.
#[test]
fn a_lower_limit_ends_the_window_and_its_grants() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let [send, platform, withdraw] = payments();
    let token = |purpose: GrantPurpose| {
        let grant = v.authorize(purpose, Some(&w), Credential::None).unwrap();
        v.redeem_grant(&grant.id, purpose.kind(), Some(&w)).unwrap()
    };
    let (send_token, platform_token, withdraw_token) =
        (token(send), token(platform), token(withdraw));
    let unredeemed = v.authorize(send, Some(&w), Credential::None).unwrap();
    set_limit(&v, 10_000_000);
    assert_eq!(
        v.signer(&w, &send_token).map(drop),
        Err(VaultError::GrantInvalid)
    );
    assert_eq!(
        v.platform_signer(
            &w,
            &platform_token,
            SignerScope::PlatformFunding { max_duffs: 1 }
        )
        .map(drop),
        Err(VaultError::GrantInvalid)
    );
    assert_eq!(
        v.masternode_withdrawal_signer(&w, &withdraw_token)
            .map(drop),
        Err(VaultError::GrantInvalid)
    );
    assert_eq!(
        v.window_lease_allowed(std::slice::from_ref(&unredeemed.id)),
        Err(VaultError::GrantInvalid)
    );
    assert_eq!(
        v.redeem_grant(&unredeemed.id, GrantKind::Spend, Some(&w))
            .map(drop),
        Err(VaultError::GrantInvalid)
    );
    every_payment_asks(&v);
    v.authorize(spend(1), Some(&w), Credential::Passphrase(PASS))
        .unwrap();
}

/// Raising the limit, or setting the one in force, ends nothing.
#[test]
fn a_higher_or_equal_limit_leaves_the_window_open() {
    const HIGHER: u64 = 100_000_000;
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let grant = v.authorize(spend(1), Some(&w), Credential::None).unwrap();
    let token = v
        .redeem_grant(&grant.id, GrantKind::Spend, Some(&w))
        .unwrap();
    let kept = v.authorize(spend(1), Some(&w), Credential::None).unwrap();
    for limit in [DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, HIGHER, HIGHER] {
        set_limit(&v, limit);
        assert!(v.signing_window_open(), "{limit}");
    }
    assert_eq!(v.signing_window_remaining(), Some(HIGHER - 2));
    v.signer(&w, &token).unwrap();
    assert!(
        v.window_lease_allowed(std::slice::from_ref(&kept.id))
            .is_ok()
    );
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
    allow_window(&v);
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

fn payments() -> [GrantPurpose; 3] {
    [
        spend(1),
        GrantPurpose::PlatformOp {
            max_duffs: 1,
            max_credits: 1,
        },
        GrantPurpose::MasternodeWithdraw {
            pro_tx_hash: [7; 32],
            max_credits: 1,
            destination: None,
        },
    ]
}

/// No credential-free way of asking for a payment gets one.
fn every_payment_asks(v: &dw_vault::Vault) {
    let w = wallet(1);
    assert!(!v.signing_window_open());
    assert_eq!(v.signing_window_remaining(), None);
    for purpose in payments() {
        assert!(!v.signing_window_covers(&[purpose]), "{purpose:?}");
        assert_eq!(
            v.authorize(purpose, Some(&w), Credential::None).map(drop),
            Err(VaultError::CredentialRequired),
            "{purpose:?}"
        );
        for set in [
            [purpose, GrantPurpose::IdentityScan],
            [GrantPurpose::SignMessage, purpose],
        ] {
            assert_eq!(
                v.authorize_set(&set, Some(&w), Credential::None).map(drop),
                Err(VaultError::CredentialRequired),
                "{set:?}"
            );
        }
    }
    assert_eq!(
        v.authorize_set(&payments(), Some(&w), Credential::None)
            .map(drop),
        Err(VaultError::CredentialRequired)
    );
}

/// "Ask for my passphrase for every payment" is the vault's rule, not only
/// the app's: whoever asks without a credential gets no payment grant, and
/// a passphrase unlock does not change that.
#[test]
fn ask_every_payment_refuses_every_credential_free_payment() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    v.store_wallet_secret(&w, &secret(1)).unwrap();
    every_payment_asks(&v);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    every_payment_asks(&v);
    // What never needs a credential still issues.
    v.authorize(GrantPurpose::IdentityScan, Some(&w), Credential::None)
        .unwrap();
    v.authorize(GrantPurpose::SignMessage, Some(&w), Credential::None)
        .unwrap();

    // The choice of another vault is not this one's.
    v.set_signing_window_allowed(&[9; 16], true);
    every_payment_asks(&v);

    allow_window(&v);
    assert!(v.signing_window_open());
    let held = v.authorize(spend(1), Some(&w), Credential::None).unwrap();
    let typed = v
        .authorize(spend(1), Some(&w), Credential::Passphrase(PASS))
        .unwrap();
    v.set_signing_window_allowed(&v.vault_id().unwrap(), false);
    every_payment_asks(&v);
    assert_eq!(
        v.window_lease_allowed(std::slice::from_ref(&held.id)),
        Err(VaultError::GrantInvalid)
    );
    assert_eq!(
        v.redeem_grant(&held.id, GrantKind::Spend, Some(&w))
            .map(drop),
        Err(VaultError::GrantInvalid)
    );
    v.unlock(PASS, UnlockScope::Full).unwrap();
    every_payment_asks(&v);
    v.redeem_grant(&typed.id, GrantKind::Spend, Some(&w))
        .unwrap();

    // A new opening of the file knows nothing of the setting until told.
    drop(v);
    let v = fx.open();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    every_payment_asks(&v);
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
    assert_eq!(
        v.window_lease_allowed(std::slice::from_ref(&id)),
        Err(VaultError::GrantInvalid)
    );
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

#[test]
fn a_passphrase_change_ends_the_signing_window() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    assert!(v.signing_window_open());
    v.change_passphrase(PASS, b"a different passphrase")
        .unwrap();
    assert_eq!(v.status().state, LockState::Unlocked);
    assert!(!v.signing_window_open());
    assert_eq!(
        v.authorize(spend(1), Some(&w), Credential::None).map(drop),
        Err(VaultError::CredentialRequired)
    );
}

/// Unlocking an unlocked vault opens a new window; the old window's grants
/// end with it, so the two cannot add up past the limit.
#[test]
fn a_second_unlock_ends_the_first_windows_grants() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let full = spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT);
    let old = v.authorize(full, Some(&w), Credential::None).unwrap();
    let typed = v
        .authorize(full, Some(&w), Credential::Passphrase(PASS))
        .unwrap();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        v.redeem_grant(&old.id, GrantKind::Spend, Some(&w))
            .map(drop),
        Err(VaultError::GrantInvalid)
    );
    let new = v.authorize(full, Some(&w), Credential::None).unwrap();
    v.redeem_grant(&new.id, GrantKind::Spend, Some(&w)).unwrap();
    v.redeem_grant(&typed.id, GrantKind::Spend, Some(&w))
        .unwrap();
}

/// In a credential-free set, only the payment is the window's: the other
/// grant neither counts against a lease nor ends with the window.
#[test]
fn only_the_payment_in_a_mixed_set_is_the_windows() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let platform = GrantPurpose::PlatformOp {
        max_duffs: 1,
        max_credits: 1,
    };
    let grants = v
        .authorize_set(
            &[platform, GrantPurpose::IdentityScan],
            Some(&w),
            Credential::None,
        )
        .unwrap();
    let ids = [grants[0].id.clone(), grants[1].id.clone()];
    assert_eq!(v.window_lease_allowed(&ids), Ok(()));
    v.end_signing_window();
    assert_eq!(
        v.check_grant(&ids[0], GrantKind::PlatformOp, Some(&w)),
        Err(VaultError::GrantInvalid)
    );
    v.check_grant(&ids[1], GrantKind::IdentityScan, Some(&w))
        .unwrap();
}

/// A token redeemed from the window issues no signer once that window has
/// ended: at its deadline, when it is closed, or when an unlock opens a new
/// one. A signer it issued before then runs to its own limits, and a
/// passphrase token is not bound to the window.
#[tokio::test]
async fn a_window_token_does_not_outlive_its_window() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let bip44 = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
    let redeem = |credential| {
        let grant = v.authorize(spend(1), Some(&w), credential).unwrap();
        v.redeem_grant(&grant.id, GrantKind::Spend, Some(&w))
            .unwrap()
    };

    let started = redeem(Credential::None);
    let signer = v.signer(&w, &started).unwrap();
    let waiting = redeem(Credential::None);
    let typed = redeem(Credential::Passphrase(PASS));
    fx.clock.advance(SIGNING_WINDOW_SECS);
    assert_eq!(
        v.signer(&w, &waiting).map(drop),
        Err(VaultError::GrantInvalid)
    );
    assert_eq!(
        v.signer(&w, &started).map(drop),
        Err(VaultError::GrantInvalid)
    );
    signer.sign_message(&bip44, b"m").await.unwrap();
    v.signer(&w, &typed)
        .unwrap()
        .sign_message(&bip44, b"m")
        .await
        .unwrap();

    let id = v.vault_id().unwrap();
    for close in 0..3 {
        v.unlock(PASS, UnlockScope::Full).unwrap();
        let waiting = redeem(Credential::None);
        match close {
            0 => v.end_signing_window(),
            1 => {
                v.set_signing_window_allowed(&id, false);
                v.set_signing_window_allowed(&id, true);
            }
            _ => {
                v.unlock(PASS, UnlockScope::Full).unwrap();
                assert!(v.signing_window_open());
            }
        }
        assert_eq!(
            v.signer(&w, &waiting).map(drop),
            Err(VaultError::GrantInvalid),
            "case {close}"
        );
    }
}

/// The wall clock moving away from the monotonic clock ends the window:
/// a suspend the monotonic clock did not count (Linux, macOS), a step
/// back, and a suspend followed by a larger step back. A step within two
/// seconds, or a wait that both clocks count, does not.
#[test]
fn the_wall_clock_leaving_the_monotonic_clock_ends_the_window() {
    type Step = fn(&TestClock);
    let cases: [(&str, Step, bool); 5] = [
        ("one second back", |c| c.rewind_wall(1), true),
        ("ten seconds back", |c| c.rewind_wall(10), false),
        ("a suspend", |c| c.forward_wall(600), false),
        (
            "a suspend, then further back",
            |c| {
                c.forward_wall(600);
                c.rewind_wall(3_600);
            },
            false,
        ),
        ("a minute's wait in both clocks", |c| c.advance(60), true),
    ];
    for (what, step, open) in cases {
        let fx = Fixture::new();
        let v = unlocked_with_wallet(&fx);
        let w = wallet(1);
        let grant = v.authorize(spend(1), Some(&w), Credential::None).unwrap();
        step(&fx.clock);
        assert_eq!(v.signing_window_open(), open, "{what}");
        assert_eq!(
            v.check_grant(&grant.id, GrantKind::Spend, Some(&w)).is_ok(),
            open,
            "{what}"
        );
        assert_eq!(
            v.authorize(spend(1), Some(&w), Credential::None).is_ok(),
            open,
            "{what}"
        );
    }
}

/// Once the host's watcher beats, a window whose last beat is more than
/// three seconds old admits nothing: a suspend that the monotonic clock
/// counted (Windows) looks like a long wait in both clocks, and only the
/// missing beats show it. A vault with no watcher keeps the old rule.
#[test]
fn a_stale_heartbeat_ends_the_window() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    fx.clock.advance(60);
    assert!(v.signing_window_open());

    v.signing_window_heartbeat();
    fx.clock.advance(3);
    assert!(v.signing_window_open());
    let grant = v.authorize(spend(1), Some(&w), Credential::None).unwrap();
    v.signing_window_heartbeat();
    fx.clock.advance(4);
    assert_eq!(
        v.authorize(spend(1), Some(&w), Credential::None).map(drop),
        Err(VaultError::CredentialRequired)
    );
    assert_eq!(
        v.check_grant(&grant.id, GrantKind::Spend, Some(&w)),
        Err(VaultError::GrantInvalid)
    );
    assert!(!v.signing_window_open());

    v.signing_window_heartbeat();
    assert!(!v.signing_window_open());
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(v.signing_window_open());
    v.authorize(spend(1), Some(&w), Credential::None).unwrap();
}

/// A passphrase change while a token redeemed from the window is still
/// out ends that token with the window: it issues no signer afterwards.
#[test]
fn a_passphrase_change_ends_an_outstanding_window_token() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    let grant = v.authorize(spend(1), Some(&w), Credential::None).unwrap();
    let token = v
        .redeem_grant(&grant.id, GrantKind::Spend, Some(&w))
        .unwrap();
    v.change_passphrase(PASS, b"a different passphrase")
        .unwrap();
    assert_eq!(v.status().state, LockState::Unlocked);
    assert!(!v.signing_window_open());
    assert_eq!(
        v.signer(&w, &token).map(drop),
        Err(VaultError::GrantInvalid)
    );
}

/// With "ask every payment", a set that mixes a payment with purposes that
/// never need a credential asks as a whole, also after a new unlock; the
/// passphrase issues all of it, and the other purposes alone still need
/// nothing.
#[test]
fn a_mixed_set_asks_when_every_payment_asks() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    v.set_signing_window_allowed(&v.vault_id().unwrap(), false);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(!v.signing_window_open());
    let platform = GrantPurpose::PlatformOp {
        max_duffs: 1,
        max_credits: 1,
    };
    let set = [
        platform,
        GrantPurpose::IdentityScan,
        GrantPurpose::SignMessage,
    ];
    assert_eq!(
        v.authorize_set(&set, Some(&w), Credential::None).map(drop),
        Err(VaultError::CredentialRequired)
    );
    assert_eq!(
        v.authorize_set(&set, Some(&w), Credential::Passphrase(PASS))
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        v.authorize_set(&set[1..], Some(&w), Credential::None)
            .unwrap()
            .len(),
        2
    );
}

/// A host that must beat (the desktop app) gets no window grant before its
/// first beat, so a watcher that never started leaves the window closed;
/// after a beat the window works, until the beats stop.
#[test]
fn a_host_that_must_beat_admits_nothing_before_its_first_beat() {
    let fx = Fixture::new();
    let v = unlocked_with_wallet(&fx);
    let w = wallet(1);
    v.require_signing_window_heartbeat();
    assert!(!v.signing_window_open());
    assert_eq!(v.signing_window_remaining(), None);
    assert_eq!(
        v.authorize(spend(1), Some(&w), Credential::None).map(drop),
        Err(VaultError::CredentialRequired)
    );
    v.signing_window_heartbeat();
    assert!(v.signing_window_open());
    v.authorize(spend(1), Some(&w), Credential::None).unwrap();
    fx.clock.advance(4);
    assert!(!v.signing_window_open());
}
