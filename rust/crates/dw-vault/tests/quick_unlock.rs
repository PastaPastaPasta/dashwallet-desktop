//! Slot B (quick unlock), the forgot-passphrase recovery and `destroy`
//! (docs/contracts/m2-engine.md §2.9; IOS-009, IOS-011, IOS-014, IOS-016,
//! IOS-109). Rust enforces the spending limit and the passphrase age, so
//! these tests are the control, not the UI.

#![allow(non_snake_case)] // test names carry checklist ids (IOS-011, …)

mod common;

use common::*;
use dw_vault::mnemonic;
use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantKind, GrantPurpose, LockState,
    PASSPHRASE_MAX_AGE_SECS, UnlockScope, Vault, VaultError,
};
use key_wallet::Network;

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn change_credential_grant(v: &Vault) -> String {
    v.authorize(
        GrantPurpose::ChangeCredential,
        None,
        Credential::Passphrase(PASS),
    )
    .expect("passphrase grant")
    .id
}

/// Encrypted vault with wallets 1 and 2, quick unlock enrolled, locked.
fn enrolled(fx: &Fixture) -> (Vault, Vec<u8>) {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    let key = v.enroll_quick_unlock(&change_credential_grant(&v)).unwrap();
    v.lock();
    (v, key.to_vec())
}

fn spend(max_duffs: u64) -> GrantPurpose {
    GrantPurpose::Spend { max_duffs }
}

#[test]
fn test_IOS_011_enrol_needs_an_encrypted_vault_and_a_passphrase_grant() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    assert_eq!(
        v.enroll_quick_unlock("nope").unwrap_err(),
        VaultError::NotEncrypted
    );

    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    assert_eq!(
        v.enroll_quick_unlock("nope").unwrap_err(),
        VaultError::GrantInvalid
    );
    // A grant for another purpose is refused and left alone.
    let reveal = v
        .authorize(
            GrantPurpose::RevealSecret,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    assert_eq!(
        v.enroll_quick_unlock(&reveal.id).unwrap_err(),
        VaultError::GrantPurposeMismatch
    );

    let key = v.enroll_quick_unlock(&change_credential_grant(&v)).unwrap();
    assert_eq!(key.len(), 32);
    assert!(v.status().quick_unlock_enrolled);
    let policy = v.quick_unlock_policy();
    assert!(policy.enrolled);
    assert_eq!(policy.spend_limit_duffs, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT);
    assert_eq!(policy.passphrase_max_age_secs, PASSPHRASE_MAX_AGE_SECS);
    assert_eq!(policy.last_passphrase_at, Some(T0));
    // The wrap key is not in the file.
    let raw = fx.raw();
    assert!(
        !raw.windows(key.len()).any(|w| w == &key[..]),
        "wrap key leaked into the vault file"
    );
    assert!(!String::from_utf8_lossy(&raw).contains(&hex::encode(&key[..])));
}

#[test]
fn test_IOS_016_quick_unlock_spends_up_to_the_limit_only() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    let w = Some(&wallet(1));

    let grant = v
        .authorize(
            spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT),
            w,
            Credential::QuickUnlock(&key),
        )
        .unwrap();
    // The lock state does not change; the grant carries its own key.
    assert_eq!(v.lock_state(), LockState::Locked);
    let token = v.redeem_grant(&grant.id, GrantKind::Spend, w).unwrap();
    v.signer(&wallet(1), &token)
        .expect("signer from the grant's key");

    assert_eq!(
        v.authorize(
            spend(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT + 1),
            w,
            Credential::QuickUnlock(&key)
        ),
        Err(VaultError::QuickUnlockLimitExceeded {
            limit_duffs: DEFAULT_QUICK_UNLOCK_SPEND_LIMIT
        })
    );
    v.authorize(GrantPurpose::SignMessage, w, Credential::QuickUnlock(&key))
        .unwrap();
}

#[test]
fn test_IOS_011_quick_unlock_never_reveals_wipes_or_changes_credentials() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    for (purpose, w) in [
        (GrantPurpose::RevealSecret, Some(wallet(1))),
        (GrantPurpose::Wipe, Some(wallet(1))),
        (GrantPurpose::ChangeCredential, None),
        (GrantPurpose::PlatformOp, Some(wallet(1))),
    ] {
        assert_eq!(
            v.authorize(purpose, w.as_ref(), Credential::QuickUnlock(&key)),
            Err(VaultError::CredentialRequired),
            "{purpose:?}"
        );
    }
    // Also while unlocked.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        v.authorize(
            GrantPurpose::RevealSecret,
            Some(&wallet(1)),
            Credential::QuickUnlock(&key)
        ),
        Err(VaultError::CredentialRequired)
    );
}

#[test]
fn test_IOS_011_wrong_or_stale_keys_are_refused() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    let w = Some(&wallet(1));
    assert_eq!(
        v.authorize(spend(1), w, Credential::QuickUnlock(&[7; 32])),
        Err(VaultError::QuickUnlockUnavailable)
    );
    assert!(matches!(
        v.authorize(spend(1), w, Credential::QuickUnlock(&[7; 5])),
        Err(VaultError::InvalidArgument(_))
    ));
    // A failed quick unlock does not count toward the passphrase throttle.
    assert_eq!(v.status().failed_attempts, 0);

    // Re-enrolling replaces the key; the old one stops working.
    let fresh = v.enroll_quick_unlock(&change_credential_grant(&v)).unwrap();
    assert_eq!(
        v.authorize(spend(1), w, Credential::QuickUnlock(&key)),
        Err(VaultError::QuickUnlockUnavailable)
    );
    v.authorize(spend(1), w, Credential::QuickUnlock(&fresh))
        .unwrap();

    // Removing slot B: unavailable; idempotent.
    assert!(!v.remove_quick_unlock().unwrap().quick_unlock_enrolled);
    v.remove_quick_unlock().unwrap();
    assert_eq!(
        v.authorize(spend(1), w, Credential::QuickUnlock(&fresh)),
        Err(VaultError::QuickUnlockUnavailable)
    );
}

#[test]
fn test_IOS_011_passphrase_older_than_seven_days_stales_quick_unlock() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    let w = Some(&wallet(1));
    fx.clock.advance(PASSPHRASE_MAX_AGE_SECS);
    v.authorize(spend(1), w, Credential::QuickUnlock(&key))
        .unwrap();
    fx.clock.advance(1);
    assert_eq!(
        v.authorize(spend(1), w, Credential::QuickUnlock(&key)),
        Err(VaultError::PassphraseStale)
    );

    // Entering the passphrase refreshes it, and the time survives a restart.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    v.lock();
    let reopened = fx.open();
    assert!(reopened.status().quick_unlock_enrolled);
    assert_eq!(
        reopened.quick_unlock_policy().last_passphrase_at,
        Some(T0 + PASSPHRASE_MAX_AGE_SECS + 1)
    );
    reopened
        .authorize(spend(1), w, Credential::QuickUnlock(&key))
        .unwrap();
}

#[test]
fn test_IOS_016_spending_limit_options_need_a_passphrase_grant() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    let w = Some(&wallet(1));
    let grant = change_credential_grant(&v);
    assert!(matches!(
        v.set_quick_unlock_spend_limit(&grant, 12_345),
        Err(VaultError::InvalidArgument(_))
    ));
    // The invalid value did not consume the grant.
    let policy = v.set_quick_unlock_spend_limit(&grant, 100_000_000).unwrap();
    assert_eq!(policy.spend_limit_duffs, 100_000_000);
    v.authorize(spend(100_000_000), w, Credential::QuickUnlock(&key))
        .unwrap();

    let policy = v
        .set_quick_unlock_spend_limit(&change_credential_grant(&v), 0)
        .unwrap();
    assert_eq!(policy.spend_limit_duffs, 0);
    assert_eq!(
        v.authorize(spend(1), w, Credential::QuickUnlock(&key)),
        Err(VaultError::QuickUnlockLimitExceeded { limit_duffs: 0 })
    );

    // The limit survives removal and re-enrolment.
    v.remove_quick_unlock().unwrap();
    v.enroll_quick_unlock(&change_credential_grant(&v)).unwrap();
    assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 0);
}

#[test]
fn test_IOS_016_editing_the_policy_in_the_file_is_detected() {
    let fx = Fixture::new();
    let (_v, key) = enrolled(&fx);
    let mut json = fx.read_json();
    json["quick_unlock"]["spend_limit_duffs"] = serde_json::json!(500_000_000u64);
    fx.write_json(&json);
    let v = fx.open();
    // The display copy shows the edit, but authorize checks the sealed one.
    assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 500_000_000);
    assert!(matches!(
        v.authorize(
            spend(500_000_000),
            Some(&wallet(1)),
            Credential::QuickUnlock(&key)
        ),
        Err(VaultError::Corrupt(_))
    ));

    // Refreshing the passphrase time in the file is detected the same way.
    let fx = Fixture::new();
    let (_v, key) = enrolled(&fx);
    fx.clock.advance(PASSPHRASE_MAX_AGE_SECS + 1);
    let mut json = fx.read_json();
    json["quick_unlock"]["last_passphrase_at"] =
        serde_json::json!(T0 + PASSPHRASE_MAX_AGE_SECS + 1);
    fx.write_json(&json);
    assert!(matches!(
        fx.open()
            .authorize(spend(1), Some(&wallet(1)), Credential::QuickUnlock(&key)),
        Err(VaultError::Corrupt(_))
    ));
}

#[test]
fn test_IOS_014_recovery_replaces_the_vault_with_the_phrase() {
    let fx = Fixture::new();
    let phrase_secret = mnemonic::derive_secret(PHRASE, b"", false).unwrap();
    let id = mnemonic::wallet_id_for_seed(&phrase_secret.seed, Network::Regtest).unwrap();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&id, &phrase_secret).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    v.enroll_quick_unlock(&change_credential_grant(&v)).unwrap();
    v.lock();
    let old_file = fx.raw();

    // The phrase must derive the wallet.
    assert_eq!(
        v.recover_with_mnemonic(&wallet(2), PHRASE, b"", OTHER),
        Err(VaultError::RecoveryMismatch)
    );
    assert_eq!(
        v.recover_with_mnemonic(&id, b"not a phrase", b"", OTHER),
        Err(VaultError::RecoveryMismatch)
    );
    assert_eq!(
        v.recover_with_mnemonic(&id, PHRASE, b"wrong 25th word", OTHER),
        Err(VaultError::RecoveryMismatch)
    );
    assert!(matches!(
        v.recover_with_mnemonic(&id, PHRASE, b"", b""),
        Err(VaultError::PassphraseRejected(_))
    ));
    assert_eq!(fx.raw(), old_file, "a refused recovery changed the vault");

    let lost = v.recover_with_mnemonic(&id, PHRASE, b"", OTHER).unwrap();
    assert_eq!(lost, vec![wallet(2)]);
    let status = v.status();
    assert_eq!(status.state, LockState::Unlocked);
    assert!(!status.quick_unlock_enrolled);
    assert_eq!(status.wallets_with_secrets, vec![id]);

    // The old file is kept next to the new one.
    let copies: Vec<_> = std::fs::read_dir(fx.vault_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("vault.dwv.replaced-")
        })
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(std::fs::read(copies[0].path()).unwrap(), old_file);

    // A fresh process: the new passphrase opens it, the old one does not.
    let reopened = fx.open();
    assert!(matches!(
        reopened.unlock(PASS, UnlockScope::Full),
        Err(VaultError::WrongPassphrase { .. })
    ));
    reopened.unlock(OTHER, UnlockScope::Full).unwrap();
    let reveal = reopened
        .authorize(
            GrantPurpose::RevealSecret,
            Some(&id),
            Credential::Passphrase(OTHER),
        )
        .unwrap();
    assert_eq!(
        &reopened.reveal_mnemonic(&id, &reveal.id).unwrap().phrase[..],
        PHRASE
    );
}

#[test]
fn test_IOS_014_recovery_needs_an_encrypted_vault() {
    let fx = Fixture::new();
    let phrase_secret = mnemonic::derive_secret(PHRASE, b"", false).unwrap();
    let id = mnemonic::wallet_id_for_seed(&phrase_secret.seed, Network::Regtest).unwrap();
    let v = fx.open();
    assert_eq!(
        v.recover_with_mnemonic(&id, PHRASE, b"", OTHER),
        Err(VaultError::NoVault)
    );
    v.create(None).unwrap();
    assert_eq!(
        v.recover_with_mnemonic(&id, PHRASE, b"", OTHER),
        Err(VaultError::NotEncrypted)
    );
}

#[test]
fn test_IOS_109_destroy_needs_an_empty_vault_and_the_passphrase() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    assert_eq!(
        v.destroy(Credential::Passphrase(PASS)),
        Err(VaultError::NotEmpty)
    );
    for n in [1, 2] {
        let wipe = v
            .authorize(
                GrantPurpose::Wipe,
                Some(&wallet(n)),
                Credential::Passphrase(PASS),
            )
            .unwrap();
        let token = v
            .redeem_grant(&wipe.id, GrantKind::Wipe, Some(&wallet(n)))
            .unwrap();
        assert!(v.wipe_wallet_secret(&wallet(n), &token).unwrap());
    }
    assert_eq!(
        v.destroy(Credential::QuickUnlock(&key)),
        Err(VaultError::CredentialRequired)
    );
    assert_eq!(
        v.destroy(Credential::None),
        Err(VaultError::CredentialRequired)
    );
    assert!(matches!(
        v.destroy(Credential::Passphrase(b"nope")),
        Err(VaultError::WrongPassphrase { .. })
    ));
    let status = v.destroy(Credential::Passphrase(PASS)).unwrap();
    assert_eq!(status.state, LockState::NoVault);
    assert!(!fx.file_path().exists());
    assert_eq!(fx.open().lock_state(), LockState::NoVault);
    // Idempotent.
    assert_eq!(
        v.destroy(Credential::None).unwrap().state,
        LockState::NoVault
    );
    // A new vault can be created afterwards.
    v.create(Some(OTHER)).unwrap();
}

#[test]
fn test_IOS_009_destroy_of_an_unencrypted_vault_deletes_the_os_store_key() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    assert_eq!(fx.os.len(), 1);
    assert_eq!(
        v.destroy(Credential::None).unwrap().state,
        LockState::NoVault
    );
    assert_eq!(fx.os.len(), 0);
    assert!(!fx.file_path().exists());
}
