//! Behaviour of the vault through its public API (review H2): passphrase
//! checks and the persisted throttle, tamper and rollback detection, encrypt
//! and change-passphrase, lock invalidation, grants (expiry, purpose, single
//! use, wallet binding, credential rules) and mixing-only unlock.

mod common;

use std::str::FromStr;

use common::*;
use dw_vault::{
    Credential, GrantKind, GrantPurpose, LockState, SignerError, UnlockScope, Vault, VaultError,
};
use key_wallet::Network;
use key_wallet::Signer;
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

const BIP44: &str = "m/44'/1'/0'/0/0";
const COINJOIN: &str = "m/9'/1'/4'/0'/0/0";

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

/// Public key at `p` derived straight from the seed of `secret(n)`.
fn expected_pubkey(n: u8, p: &str) -> dashcore::secp256k1::PublicKey {
    let master = ExtendedPrivKey::new_master(Network::Regtest, &[n; 64]).unwrap();
    let key = master.derive_priv(&path(p)).unwrap();
    dashcore::secp256k1::PublicKey::from_secret_key(&key.private_key)
}

/// Encrypted vault holding wallets 1 and 2, left locked.
fn locked_vault(fx: &Fixture) -> Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    v.lock();
    v
}

fn spend() -> GrantPurpose {
    GrantPurpose::Spend { max_duffs: 1_000 }
}

// MARK: passphrase and throttle

#[test]
fn wrong_passphrase_counts_and_throttles_across_restart() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);

    for expected in 1..=2 {
        match v.unlock(b"nope", UnlockScope::Full) {
            Err(VaultError::WrongPassphrase {
                failed_attempts,
                retry_after_secs,
            }) => {
                assert_eq!(failed_attempts, expected);
                assert_eq!(retry_after_secs, None);
            }
            other => panic!("attempt {expected}: {other:?}"),
        }
    }
    assert_eq!(
        v.unlock(b"nope", UnlockScope::Full),
        Err(VaultError::WrongPassphrase {
            failed_attempts: 3,
            retry_after_secs: Some(60),
        })
    );
    // Throttled even with the right passphrase; no attempt is counted.
    assert_eq!(
        v.unlock(PASS, UnlockScope::Full),
        Err(VaultError::Throttled {
            retry_after_secs: 60
        })
    );
    assert_eq!(v.status().failed_attempts, 3);

    // A restart reads the counter from disk.
    fx.clock.advance(20);
    let restarted = fx.open();
    let status = restarted.status();
    assert_eq!(status.state, LockState::Locked);
    assert_eq!(status.failed_attempts, 3);
    assert_eq!(status.retry_after_secs, Some(40));
    assert_eq!(
        restarted.unlock(PASS, UnlockScope::Full),
        Err(VaultError::Throttled {
            retry_after_secs: 40
        })
    );
    assert_eq!(
        restarted.authorize(spend(), Some(&wallet(1)), Credential::Passphrase(PASS)),
        Err(VaultError::Throttled {
            retry_after_secs: 40
        })
    );
    assert!(matches!(
        restarted.change_passphrase(PASS, OTHER),
        Err(VaultError::Throttled { .. })
    ));

    // After the wait the right passphrase works and resets the counter on disk.
    fx.clock.advance(40);
    restarted.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(restarted.status().failed_attempts, 0);
    assert_eq!(fx.open().status().failed_attempts, 0);
    assert_eq!(fx.read_json()["throttle"]["failed_attempts"], 0);
}

#[test]
fn passphrase_operations_on_an_unencrypted_vault() {
    let fx = Fixture::new();
    let v = fx.open();
    assert_eq!(v.unlock(PASS, UnlockScope::Full), Err(VaultError::NoVault));
    v.create(None).unwrap();
    assert_eq!(v.lock_state(), LockState::NoKeys);
    assert_eq!(
        v.unlock(PASS, UnlockScope::Full),
        Err(VaultError::NotEncrypted)
    );
    assert_eq!(
        v.change_passphrase(PASS, OTHER),
        Err(VaultError::NotEncrypted)
    );
    assert_eq!(
        v.authorize(spend(), Some(&wallet(1)), Credential::Passphrase(PASS)),
        Err(VaultError::NotEncrypted)
    );
    assert_eq!(v.create(None), Err(VaultError::AlreadyExists));
    // A second handle on the same directory sees the file.
    assert_eq!(fx.open().create(Some(PASS)), Err(VaultError::AlreadyExists));
}

// MARK: tamper and rollback

/// Tampers with the vault file of a locked vault, restarts, and returns
/// what unlocking reports.
fn unlock_after(fx: &Fixture, tamper: impl FnOnce(&mut serde_json::Value)) -> VaultError {
    let mut json = fx.read_json();
    tamper(&mut json);
    fx.write_json(&json);
    fx.open()
        .unlock(PASS, UnlockScope::Full)
        .expect_err("tampered vault unlocked")
}

fn seed_record(n: u8) -> String {
    format!("wallet/{}/seed", hex::encode(wallet(n)))
}

#[test]
fn tampered_record_is_detected() {
    let fx = Fixture::new();
    locked_vault(&fx);
    let err = unlock_after(&fx, |j| {
        let ct = j["records"][seed_record(1)]["ct"]
            .as_str()
            .unwrap()
            .to_owned();
        j["records"][seed_record(1)]["ct"] = flip_hex(&ct).into();
    });
    assert!(
        matches!(err, VaultError::Corrupt(ref d) if d.contains("differs from the manifest")),
        "{err:?}"
    );
}

#[test]
fn deleted_or_swapped_records_are_detected() {
    let fx = Fixture::new();
    locked_vault(&fx);
    let original = fx.raw();

    let err = unlock_after(&fx, |j| {
        j["records"]
            .as_object_mut()
            .unwrap()
            .remove(&seed_record(2));
    });
    assert!(
        matches!(err, VaultError::Corrupt(ref d) if d.contains("record set")),
        "{err:?}"
    );

    fx.restore_raw(&original);
    let err = unlock_after(&fx, |j| {
        let one = j["records"][seed_record(1)].clone();
        let two = j["records"][seed_record(2)].clone();
        j["records"][seed_record(1)] = two;
        j["records"][seed_record(2)] = one;
    });
    assert!(matches!(err, VaultError::Corrupt(_)), "{err:?}");

    fx.restore_raw(&original);
    fx.open().unlock(PASS, UnlockScope::Full).unwrap();
}

#[test]
fn tampered_manifest_is_detected() {
    let fx = Fixture::new();
    locked_vault(&fx);
    let err = unlock_after(&fx, |j| {
        let ct = j["manifest"]["ct"].as_str().unwrap().to_owned();
        j["manifest"]["ct"] = flip_hex(&ct).into();
    });
    assert_eq!(
        err,
        VaultError::Corrupt("manifest failed authentication".into())
    );
}

/// The wrapped key and the KDF parameters are authenticated; a changed byte
/// fails exactly like a wrong passphrase (the AEAD cannot tell them apart)
/// and counts toward the throttle.
#[test]
fn tampered_wrapped_key_or_kdf_params_fail_to_unwrap() {
    let fx = Fixture::new();
    locked_vault(&fx);
    let original = fx.raw();

    let err = unlock_after(&fx, |j| {
        let ct = j["slot_p"]["wrapped_dek"]["ct"]
            .as_str()
            .unwrap()
            .to_owned();
        j["slot_p"]["wrapped_dek"]["ct"] = flip_hex(&ct).into();
    });
    assert!(
        matches!(
            err,
            VaultError::WrongPassphrase {
                failed_attempts: 1,
                ..
            }
        ),
        "{err:?}"
    );

    fx.restore_raw(&original);
    let err = unlock_after(&fx, |j| {
        let salt = j["slot_p"]["salt"].as_str().unwrap().to_owned();
        j["slot_p"]["salt"] = flip_hex(&salt).into();
    });
    assert!(matches!(err, VaultError::WrongPassphrase { .. }), "{err:?}");
}

#[test]
fn record_rolled_back_to_an_older_ciphertext_is_detected() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    let old = fx.read_json()["records"][seed_record(1)].clone();
    // Re-storing the same wallet seals the records again (new nonces).
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.lock();

    let err = unlock_after(&fx, |j| j["records"][seed_record(1)] = old);
    assert!(matches!(err, VaultError::Corrupt(_)), "{err:?}");
}

/// An older but self-consistent vault file put on disk while the app runs
/// is never adopted: the vault keeps its newer in-memory generation, and its
/// next write puts every record back.
#[test]
fn file_rolled_back_on_disk_is_not_adopted() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    let older = fx.raw();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    fx.restore_raw(&older);

    // Passphrase checks and grants use the in-memory file.
    v.change_passphrase(PASS, OTHER).unwrap();
    v.lock();
    v.unlock(OTHER, UnlockScope::Full).unwrap();
    assert_eq!(v.status().wallets_with_secrets, vec![wallet(1), wallet(2)]);
    v.seed_derivation(&wallet(2)).unwrap();

    let reopened = fx.open();
    assert_eq!(
        reopened.status().wallets_with_secrets,
        vec![wallet(1), wallet(2)]
    );
    reopened.unlock(OTHER, UnlockScope::Full).unwrap();
}

#[test]
fn tampered_unencrypted_vault_is_detected() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    let mut j = fx.read_json();
    let ct = j["manifest"]["ct"].as_str().unwrap().to_owned();
    j["manifest"]["ct"] = flip_hex(&ct).into();
    fx.write_json(&j);

    let restarted = fx.open();
    assert_eq!(restarted.lock_state(), LockState::Unencrypted);
    assert!(matches!(
        restarted.seed_derivation(&wallet(1)),
        Err(VaultError::Corrupt(_))
    ));
}

// MARK: encrypt and change passphrase

#[test]
fn encrypt_adds_slot_p_and_deletes_slot_o() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    assert_eq!(fx.os.len(), 1);
    assert_eq!(v.lock_state(), LockState::Unencrypted);

    // A grant of another purpose does not encrypt and is left in place.
    let wrong = v
        .authorize(
            GrantPurpose::SignMessage,
            Some(&wallet(1)),
            Credential::None,
        )
        .unwrap();
    assert_eq!(
        v.encrypt(PASS, &wrong.id),
        Err(VaultError::GrantPurposeMismatch)
    );
    let grant = v
        .authorize(GrantPurpose::ChangeCredential, None, Credential::None)
        .unwrap();
    let status = v.encrypt(PASS, &grant.id).unwrap();
    assert_eq!(status.state, LockState::Locked);
    assert!(status.encrypted);
    assert_eq!(fx.os.len(), 0);
    let json = fx.read_json();
    assert!(json["slot_o"].is_null());
    assert!(json["slot_p"].is_object());

    // The grant was consumed; the vault cannot be encrypted twice.
    assert_eq!(
        v.encrypt(OTHER, &grant.id),
        Err(VaultError::AlreadyEncrypted)
    );
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let revealed = reveal(&v, 1, Credential::Passphrase(PASS)).unwrap();
    assert_eq!(&revealed.phrase[..], b"phrase-1");

    let restarted = fx.open();
    assert_eq!(restarted.lock_state(), LockState::Locked);
    restarted.unlock(PASS, UnlockScope::Full).unwrap();
    restarted.seed_derivation(&wallet(1)).unwrap();
}

/// DEC-134's split: the check changes nothing, and an apply after slot P
/// changed meanwhile is refused without counting an attempt.
#[test]
fn a_checked_passphrase_change_applies_only_to_the_slot_it_checked() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);
    let checked = v.check_passphrase_change(PASS, OTHER).unwrap();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    v.lock();
    v.apply_passphrase_change(checked).unwrap();
    v.unlock(OTHER, UnlockScope::Full).unwrap();

    let stale = v.check_passphrase_change(OTHER, b"third").unwrap();
    v.change_passphrase(OTHER, PASS).unwrap();
    assert!(matches!(
        v.apply_passphrase_change(stale),
        Err(VaultError::WrongPassphrase {
            failed_attempts: 0,
            ..
        })
    ));
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert!(matches!(
        v.check_passphrase_change(b"wrong", OTHER),
        Err(VaultError::WrongPassphrase { .. })
    ));
}

#[test]
fn change_passphrase_keeps_records_and_lock_state() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);
    let before = fx.read_json()["records"].clone();

    let status = v.change_passphrase(PASS, OTHER).unwrap();
    assert_eq!(status.state, LockState::Locked);
    assert_eq!(fx.read_json()["records"], before);
    assert!(matches!(
        v.unlock(PASS, UnlockScope::Full),
        Err(VaultError::WrongPassphrase { .. })
    ));
    v.unlock(OTHER, UnlockScope::Full).unwrap();
    assert_eq!(
        &reveal(&v, 2, Credential::Passphrase(OTHER)).unwrap().phrase[..],
        b"phrase-2"
    );

    // Unlocked stays unlocked.
    assert_eq!(
        v.change_passphrase(OTHER, PASS).unwrap().state,
        LockState::Unlocked
    );
    assert!(matches!(
        v.change_passphrase(b"", PASS),
        Err(VaultError::WrongPassphrase { .. })
    ));
    assert!(matches!(
        v.change_passphrase(PASS, b""),
        Err(VaultError::PassphraseRejected(_))
    ));

    let restarted = fx.open();
    restarted.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        restarted.status().wallets_with_secrets,
        vec![wallet(1), wallet(2)]
    );
}

// MARK: grants

fn reveal(
    v: &Vault,
    n: u8,
    credential: Credential<'_>,
) -> Result<dw_vault::RevealedMnemonic, VaultError> {
    let grant = v.authorize(GrantPurpose::RevealSecret, Some(&wallet(n)), credential)?;
    v.reveal_mnemonic(&wallet(n), &grant.id)
}

#[test]
fn grants_expire_are_single_use_and_keep_their_purpose() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);

    let g = v
        .authorize(GrantPurpose::SignMessage, Some(&w), Credential::None)
        .unwrap();
    assert_eq!(g.expires_at, T0 + TTL);
    assert_eq!(g.wallet, Some(w));
    assert!(g.single_use);
    // Purpose mismatch leaves the grant in place.
    assert_eq!(
        v.redeem_grant(&g.id, GrantKind::Spend, Some(&w))
            .unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    // Still valid at the expiry second.
    fx.clock.advance(TTL);
    let token = v
        .redeem_grant(&g.id, GrantKind::SignMessage, Some(&w))
        .unwrap();
    assert_eq!(token.purpose(), GrantPurpose::SignMessage);
    assert_eq!(token.wallet(), Some(w));
    // Single use.
    assert_eq!(
        v.redeem_grant(&g.id, GrantKind::SignMessage, Some(&w))
            .unwrap_err(),
        VaultError::GrantInvalid
    );

    let late = v
        .authorize(GrantPurpose::SignMessage, Some(&w), Credential::None)
        .unwrap();
    fx.clock.advance(TTL + 1);
    assert_eq!(
        v.redeem_grant(&late.id, GrantKind::SignMessage, Some(&w))
            .unwrap_err(),
        VaultError::GrantInvalid
    );

    let revoked = v
        .authorize(GrantPurpose::SignMessage, Some(&w), Credential::None)
        .unwrap();
    v.revoke_grant(&revoked.id);
    v.revoke_grant("unknown");
    assert_eq!(
        v.redeem_grant(&revoked.id, GrantKind::SignMessage, Some(&w))
            .unwrap_err(),
        VaultError::GrantInvalid
    );
}

/// `grant_purpose` reads a grant's caps without consuming it (DP1-03: a
/// registration refused for its cost keeps its grant).
#[test]
fn grant_purpose_reads_the_caps_and_leaves_the_grant() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    let w = wallet(1);
    let op = GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 7,
    };
    let g = v.authorize(op, Some(&w), Credential::None).unwrap();
    for _ in 0..2 {
        assert_eq!(
            v.grant_purpose(&g.id, GrantKind::PlatformOp, Some(&w)),
            Ok(op)
        );
    }
    assert_eq!(
        v.grant_purpose(&g.id, GrantKind::Spend, Some(&w)),
        Err(VaultError::GrantPurposeMismatch)
    );
    assert_eq!(
        v.grant_purpose(&g.id, GrantKind::PlatformOp, Some(&wallet(2))),
        Err(VaultError::GrantPurposeMismatch)
    );
    let token = v
        .redeem_grant(&g.id, GrantKind::PlatformOp, Some(&w))
        .unwrap();
    assert_eq!(token.max_credits(), Some(7));
    assert_eq!(
        v.grant_purpose(&g.id, GrantKind::PlatformOp, Some(&w)),
        Err(VaultError::GrantInvalid)
    );
    // A grant revoked by the lock reports the lock.
    let g = v.authorize(op, Some(&w), Credential::None).unwrap();
    v.lock();
    assert_eq!(
        v.grant_purpose(&g.id, GrantKind::PlatformOp, Some(&w)),
        Err(VaultError::Locked)
    );
}

/// Review M-6: wallet-scoped grants name their wallet and work only on it.
#[test]
fn grants_are_bound_to_their_wallet() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();

    assert!(matches!(
        v.authorize(spend(), None, Credential::None),
        Err(VaultError::InvalidArgument(_))
    ));
    assert!(matches!(
        v.authorize(
            GrantPurpose::ChangeCredential,
            Some(&wallet(1)),
            Credential::Passphrase(PASS)
        ),
        Err(VaultError::InvalidArgument(_))
    ));
    // A malformed request does not count as a passphrase attempt.
    assert!(matches!(
        v.authorize(
            GrantPurpose::RevealSecret,
            None,
            Credential::Passphrase(b"x")
        ),
        Err(VaultError::InvalidArgument(_))
    ));
    assert_eq!(v.status().failed_attempts, 0);

    let g = v
        .authorize(
            GrantPurpose::RevealSecret,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    assert_eq!(
        v.reveal_mnemonic(&wallet(2), &g.id).unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    assert_eq!(
        &v.reveal_mnemonic(&wallet(1), &g.id).unwrap().phrase[..],
        b"phrase-1"
    );

    let s = v
        .authorize(spend(), Some(&wallet(1)), Credential::None)
        .unwrap();
    assert_eq!(
        v.check_grant(&s.id, GrantKind::Spend, Some(&wallet(2)))
            .unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    let token = v
        .redeem_grant(&s.id, GrantKind::Spend, Some(&wallet(1)))
        .unwrap();
    assert_eq!(token.max_duffs(), Some(1_000));
    assert_eq!(
        v.signer(&wallet(2), &token).unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    v.signer(&wallet(1), &token).unwrap();

    let wipe = v
        .authorize(
            GrantPurpose::Wipe,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    let wipe = v
        .redeem_grant(&wipe.id, GrantKind::Wipe, Some(&wallet(1)))
        .unwrap();
    assert_eq!(
        v.wipe_wallet_secret(&wallet(2), &wipe).unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    assert!(v.wipe_wallet_secret(&wallet(1), &wipe).unwrap());
    assert_eq!(v.status().wallets_with_secrets, vec![wallet(2)]);
}

/// Review M5: reveal, wipe and credential change need the passphrase on an
/// encrypted vault even while it is unlocked; spending and signing do not.
#[test]
fn reveal_wipe_and_credential_change_need_the_passphrase_when_encrypted() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    assert_eq!(v.lock_state(), LockState::Unlocked);
    let w = Some(&wallet(1));

    for (purpose, wallet) in [
        (GrantPurpose::RevealSecret, w),
        (GrantPurpose::Wipe, w),
        (GrantPurpose::ChangeCredential, None),
    ] {
        assert_eq!(
            v.authorize(purpose, wallet, Credential::None),
            Err(VaultError::CredentialRequired),
            "{purpose:?} unlocked"
        );
        v.authorize(purpose, wallet, Credential::Passphrase(PASS))
            .unwrap();
    }
    for purpose in [
        spend(),
        GrantPurpose::SignMessage,
        GrantPurpose::PlatformOp {
            max_duffs: 1,
            max_credits: 1,
        },
        GrantPurpose::IdentityScan,
    ] {
        v.authorize(purpose, w, Credential::None).unwrap();
    }

    v.lock();
    assert_eq!(
        v.authorize(GrantPurpose::RevealSecret, w, Credential::None),
        Err(VaultError::CredentialRequired)
    );
    assert_eq!(
        v.authorize(spend(), w, Credential::None),
        Err(VaultError::Locked)
    );
    assert_eq!(
        v.authorize(spend(), w, Credential::QuickUnlock(b"k")),
        Err(VaultError::QuickUnlockUnavailable)
    );

    // Unencrypted: no passphrase exists, so no credential is needed.
    let fx2 = Fixture::new();
    let u = fx2.open();
    u.create(None).unwrap();
    u.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    assert_eq!(
        &reveal(&u, 1, Credential::None).unwrap().phrase[..],
        b"phrase-1"
    );
    u.authorize(GrantPurpose::ChangeCredential, None, Credential::None)
        .unwrap();
}

// MARK: lock

#[tokio::test]
async fn lock_invalidates_grants_tokens_and_signers() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    let w = Some(&wallet(1));

    let pending = v.authorize(spend(), w, Credential::None).unwrap();
    let redeemed = v.authorize(spend(), w, Credential::None).unwrap();
    let token = v.redeem_grant(&redeemed.id, GrantKind::Spend, w).unwrap();
    let for_signer = v.authorize(spend(), w, Credential::None).unwrap();
    let signer_token = v.redeem_grant(&for_signer.id, GrantKind::Spend, w).unwrap();
    let signer = v.signer(&wallet(1), &signer_token).unwrap();
    let mixing = v.mixing_signer(&wallet(1)).unwrap();
    assert_eq!(
        signer.public_key(&path(BIP44)).await.unwrap(),
        expected_pubkey(1, BIP44)
    );

    assert_eq!(v.lock().state, LockState::Locked);
    assert_eq!(
        v.redeem_grant(&pending.id, GrantKind::Spend, w)
            .unwrap_err(),
        VaultError::GrantInvalid
    );
    assert_eq!(
        v.check_grant(&pending.id, GrantKind::Spend, w).unwrap_err(),
        VaultError::Locked
    );
    assert_eq!(
        v.signer(&wallet(1), &token).unwrap_err(),
        VaultError::Locked
    );
    assert_eq!(
        signer.public_key(&path(BIP44)).await.unwrap_err(),
        SignerError::Locked
    );
    assert_eq!(
        mixing.public_key(&path(COINJOIN)).await.unwrap_err(),
        SignerError::Locked
    );

    // Unlocking again does not revive them.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        signer.public_key(&path(BIP44)).await.unwrap_err(),
        SignerError::Locked
    );
    assert_eq!(
        v.signer(&wallet(1), &token).unwrap_err(),
        VaultError::GrantInvalid
    );
}

// MARK: passphrase grants keep the lock state (review M4)

#[tokio::test]
async fn passphrase_grant_on_a_locked_vault_leaves_it_locked() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);
    let w = Some(&wallet(1));

    let g = v
        .authorize(spend(), w, Credential::Passphrase(PASS))
        .unwrap();
    assert_eq!(v.lock_state(), LockState::Locked);
    // The grant carries its own key, so the early check passes while locked;
    // for another wallet it reports the mismatch, not the lock.
    v.check_grant(&g.id, GrantKind::Spend, w).unwrap();
    assert_eq!(
        v.check_grant(&g.id, GrantKind::Spend, Some(&wallet(2)))
            .unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    let token = v.redeem_grant(&g.id, GrantKind::Spend, w).unwrap();
    let signer = v.signer(&wallet(1), &token).unwrap();
    assert_eq!(
        signer.public_key(&path(BIP44)).await.unwrap(),
        expected_pubkey(1, BIP44)
    );
    assert_eq!(v.lock_state(), LockState::Locked);
    // The vault itself still has no key.
    assert_eq!(
        v.seed_derivation(&wallet(1)).unwrap_err(),
        VaultError::Locked
    );
    assert_eq!(
        v.store_wallet_secret(&wallet(3), &secret(3)).unwrap_err(),
        VaultError::Locked
    );

    // Reveal and wipe work under their own grants and leave it locked too.
    assert_eq!(
        &reveal(&v, 2, Credential::Passphrase(PASS)).unwrap().phrase[..],
        b"phrase-2"
    );
    let wipe = v
        .authorize(GrantPurpose::Wipe, w, Credential::Passphrase(PASS))
        .unwrap();
    let wipe = v.redeem_grant(&wipe.id, GrantKind::Wipe, w).unwrap();
    assert!(v.wipe_wallet_secret(&wallet(1), &wipe).unwrap());
    assert_eq!(v.lock_state(), LockState::Locked);
    assert_eq!(v.status().wallets_with_secrets, vec![wallet(2)]);

    // lock() also stops a signer that holds its grant's key.
    v.lock();
    assert_eq!(
        signer.public_key(&path(BIP44)).await.unwrap_err(),
        SignerError::Locked
    );
}

#[tokio::test]
async fn passphrase_grant_on_a_mixing_only_vault_keeps_mixing_only() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    let w = Some(&wallet(1));

    let g = v
        .authorize(spend(), w, Credential::Passphrase(PASS))
        .unwrap();
    assert_eq!(v.lock_state(), LockState::UnlockedMixingOnly);
    let token = v.redeem_grant(&g.id, GrantKind::Spend, w).unwrap();
    let signer = v.signer(&wallet(1), &token).unwrap();
    // Full scope for this grant's signer only.
    assert_eq!(
        signer.public_key(&path(BIP44)).await.unwrap(),
        expected_pubkey(1, BIP44)
    );
    assert_eq!(v.lock_state(), LockState::UnlockedMixingOnly);
    assert_eq!(
        v.authorize(spend(), w, Credential::None),
        Err(VaultError::MixingOnly)
    );
}

#[test]
fn passphrase_grant_on_an_unlocked_vault_keeps_it_unlocked() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    v.authorize(spend(), Some(&wallet(1)), Credential::Passphrase(PASS))
        .unwrap();
    assert_eq!(v.lock_state(), LockState::Unlocked);
    // A wrong passphrase is refused even though the vault is unlocked.
    assert!(matches!(
        v.authorize(spend(), Some(&wallet(1)), Credential::Passphrase(b"no")),
        Err(VaultError::WrongPassphrase { .. })
    ));
}

// MARK: mixing-only unlock

#[tokio::test]
async fn mixing_only_unlock_limits_keys_to_the_coinjoin_account() {
    let fx = Fixture::new();
    let v = locked_vault(&fx);
    assert_eq!(
        v.unlock(PASS, UnlockScope::MixingOnly).unwrap().state,
        LockState::UnlockedMixingOnly
    );

    // Everything that needs the full key refuses.
    assert_eq!(
        v.seed_derivation(&wallet(1)).unwrap_err(),
        VaultError::MixingOnly
    );
    assert_eq!(
        v.store_wallet_secret(&wallet(3), &secret(3)).unwrap_err(),
        VaultError::MixingOnly
    );
    assert_eq!(
        v.authorize(
            GrantPurpose::SignMessage,
            Some(&wallet(1)),
            Credential::None
        ),
        Err(VaultError::MixingOnly)
    );

    let mixing = v.mixing_signer(&wallet(1)).unwrap();
    assert_eq!(mixing.scope(), dw_vault::SignerScope::CoinJoinOnly);
    assert_eq!(
        mixing.public_key(&path(COINJOIN)).await.unwrap(),
        expected_pubkey(1, COINJOIN)
    );
    assert!(matches!(
        mixing.public_key(&path(BIP44)).await,
        Err(SignerError::PathNotAllowed(_))
    ));
    assert!(matches!(
        mixing.sign_ecdsa(&path(BIP44), [7; 32]).await,
        Err(SignerError::PathNotAllowed(_))
    ));
    mixing.sign_ecdsa(&path(COINJOIN), [7; 32]).await.unwrap();
    assert_eq!(
        v.mixing_signer(&wallet(9)).unwrap_err(),
        VaultError::NoSecret
    );

    // QT-112: mixing makes denominations from BIP44 coins under the
    // mixing-only unlock, through the funding scope.
    let funding = v.mixing_funding_signer(&wallet(1)).unwrap();
    assert_eq!(funding.scope(), dw_vault::SignerScope::CoinJoinFunding);
    funding.sign_ecdsa(&path(BIP44), [7; 32]).await.unwrap();
    funding.sign_ecdsa(&path(COINJOIN), [7; 32]).await.unwrap();
    assert_eq!(
        funding.public_key(&path(BIP44)).await.unwrap(),
        expected_pubkey(1, BIP44)
    );

    // Upgrading to a full unlock changes the scope and invalidates the
    // mixing signer (new epoch).
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        mixing.public_key(&path(COINJOIN)).await.unwrap_err(),
        SignerError::Locked
    );
    v.seed_derivation(&wallet(1)).unwrap();

    v.lock();
    assert_eq!(v.mixing_signer(&wallet(1)).unwrap_err(), VaultError::Locked);
}

/// Review M1: a crafted `vault.dwv` with an absurd Argon2id cost is refused
/// as corrupt (no 4 TiB allocation, no hours of hashing) and the attempt is
/// not counted.
#[test]
fn crafted_kdf_cost_in_the_vault_file_is_corrupt() {
    let fx = Fixture::new();
    locked_vault(&fx);
    let original = fx.raw();
    for (field, value) in [
        ("m_kib", u64::from(u32::MAX)),
        ("t", u64::from(u32::MAX)),
        ("p", 17),
    ] {
        fx.restore_raw(&original);
        let err = unlock_after(&fx, |j| j["slot_p"]["kdf"][field] = value.into());
        assert!(
            matches!(err, VaultError::Corrupt(ref d) if d.contains("above the limits")),
            "{field}: {err:?}"
        );
        assert_eq!(fx.open().status().failed_attempts, 0);
    }
}
