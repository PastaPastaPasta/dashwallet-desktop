//! E0-04 P1 grants (E0-04 design §3, §12 "dw-vault"): `PlatformOp` caps,
//! `IdentityScan`, `authorize_set`, `KeyHold`, `Vault::epoch()` and
//! `QuickUnlock` for `PlatformOp` (DEC-67). The lock-race style `KeyHold`
//! tests (an operation in flight, 16 signers and 300 drops) are in
//! `src/lock_race_tests.rs`, which can pause an operation inside the gate.

mod common;

use std::str::FromStr;

use common::*;
use dw_vault::{
    CREDITS_PER_DUFF, Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantPurpose, GrantToken,
    LockState, PASSPHRASE_MAX_AGE_SECS, SignerError, SignerScope, UnlockScope, Vault, VaultError,
    WalletSigner,
};
use key_wallet::Signer;
use key_wallet::bip32::DerivationPath;

const IDENTITY_KEY: &str = "m/9'/1'/5'/0'/0'/0'/0'";
const BIP44: &str = "m/44'/1'/0'/0/0";

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

fn platform_op(max_duffs: u64, max_credits: u64) -> GrantPurpose {
    GrantPurpose::PlatformOp {
        max_duffs,
        max_credits,
    }
}

/// An encrypted vault holding wallet 1, unlocked.
fn encrypted(fx: &Fixture) -> Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

/// A redeemed grant of `purpose` for wallet 1.
fn token(v: &Vault, purpose: GrantPurpose, credential: Credential<'_>) -> GrantToken {
    let grant = v.authorize(purpose, Some(&wallet(1)), credential).unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
        .unwrap()
}

/// `grants`, redeemed in order.
fn redeem_all(v: &Vault, grants: &[dw_vault::AuthGrant]) -> Vec<GrantToken> {
    grants
        .iter()
        .map(|g| {
            v.redeem_grant(&g.id, g.purpose.kind(), Some(&wallet(1)))
                .unwrap()
        })
        .collect()
}

// MARK: PlatformOp caps (§3.1, §3.4)

#[test]
fn platform_signer_scopes_are_capped_by_the_token() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let w = wallet(1);
    let funding = |max_duffs| SignerScope::PlatformFunding { max_duffs };
    let mismatch = Err(VaultError::GrantPurposeMismatch);

    // Funding up to the token's cap, and not one duff more.
    let t = token(&v, platform_op(1_000, 5), Credential::None);
    assert_eq!(t.max_duffs(), Some(1_000));
    assert_eq!(t.max_credits(), Some(5));
    for wanted in [0, 1, 999, 1_000] {
        assert_eq!(
            v.platform_signer(&w, &t, funding(wanted)).unwrap().scope(),
            funding(wanted)
        );
    }
    for wanted in [1_001, u64::MAX] {
        assert_eq!(
            v.platform_signer(&w, &t, funding(wanted)).map(drop),
            mismatch,
            "{wanted}"
        );
    }
    v.platform_signer(&w, &t, SignerScope::PlatformIdentity)
        .unwrap();

    // `max_duffs = 0`: no funding signer at all.
    let t = token(&v, platform_op(0, 5), Credential::None);
    for wanted in [0, 1] {
        assert_eq!(
            v.platform_signer(&w, &t, funding(wanted)).map(drop),
            mismatch
        );
    }
    v.platform_signer(&w, &t, SignerScope::PlatformIdentity)
        .unwrap();

    // `max_credits = 0`: no identity signer.
    let t = token(&v, platform_op(1_000, 0), Credential::None);
    assert_eq!(
        v.platform_signer(&w, &t, SignerScope::PlatformIdentity)
            .map(drop),
        mismatch
    );
    v.platform_signer(&w, &t, funding(1_000)).unwrap();

    // Contact crypto never hands off, so any `PlatformOp` yields it, even
    // one that grants nothing else.
    let t = token(&v, platform_op(0, 0), Credential::None);
    v.platform_signer(&w, &t, SignerScope::DashPayCrypto)
        .unwrap();
    assert_eq!(
        v.platform_signer(&w, &t, SignerScope::PlatformIdentity)
            .map(drop),
        mismatch
    );
    assert_eq!(v.platform_signer(&w, &t, funding(0)).map(drop), mismatch);
}

// MARK: IdentityScan (§3.2)

/// `scan_key` takes an `IdentityScan` token and refuses a `PlatformOp` one,
/// whatever its caps, in every state of m1-engine §2.2's credential table;
/// each grant needs what the table's `Spend` column says.
#[test]
fn scan_key_takes_identity_scan_only_in_every_vault_state() {
    let w = Some(&wallet(1));
    let scan = GrantPurpose::IdentityScan;
    let capped = platform_op(u64::MAX, u64::MAX);

    // NoVault: no grant at all.
    let fx = Fixture::new();
    let v = fx.open();
    assert_eq!(v.lock_state(), LockState::NoVault);
    for purpose in [scan, capped] {
        assert_eq!(
            v.authorize(purpose, w, Credential::None).map(drop),
            Err(VaultError::NoVault)
        );
    }

    // NoKeys: the grant needs no credential; the wallet has no seed.
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    assert_eq!(v.lock_state(), LockState::NoKeys);
    assert_eq!(
        v.scan_key(&wallet(1), &token(&v, scan, Credential::None))
            .map(drop),
        Err(VaultError::NoSecret)
    );
    assert_eq!(
        v.scan_key(&wallet(1), &token(&v, capped, Credential::None))
            .map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );

    // Unencrypted: no credential; a passphrase is `NotEncrypted`.
    let fx = Fixture::new();
    let v = fx.open();
    v.create(None).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    assert_eq!(v.lock_state(), LockState::Unencrypted);
    assert_eq!(
        v.authorize(scan, w, Credential::Passphrase(PASS)).map(drop),
        Err(VaultError::NotEncrypted)
    );
    assert_scan_only(&v, Credential::None);

    // Unlocked: no credential, or the passphrase.
    let fx = Fixture::new();
    let v = encrypted(&fx);
    assert_eq!(v.lock_state(), LockState::Unlocked);
    assert_scan_only(&v, Credential::None);
    assert_scan_only(&v, Credential::Passphrase(PASS));

    // Locked and mixing-only: the passphrase, else the state's error.
    v.lock();
    assert_eq!(v.lock_state(), LockState::Locked);
    assert_eq!(
        v.authorize(scan, w, Credential::None).map(drop),
        Err(VaultError::Locked)
    );
    assert_scan_only(&v, Credential::Passphrase(PASS));
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    assert_eq!(v.lock_state(), LockState::UnlockedMixingOnly);
    assert_eq!(
        v.authorize(scan, w, Credential::None).map(drop),
        Err(VaultError::MixingOnly)
    );
    assert_scan_only(&v, Credential::Passphrase(PASS));
}

/// With `credential`, an `IdentityScan` token releases the master key (its
/// key held first when it carries its own) and no `PlatformOp` token does.
fn assert_scan_only(v: &Vault, credential: Credential<'_>) {
    let state = v.lock_state();
    let mut scan = [token(v, GrantPurpose::IdentityScan, credential)];
    let key = match v.hold_key(&mut scan).unwrap() {
        Some(hold) => {
            let key = v.scan_key_held(&wallet(1), &hold, &scan[0]).unwrap();
            assert_eq!(key.master_key().unwrap().depth, 0, "{state:?}");
            drop(hold);
            assert_eq!(key.master_key().map(drop), Err(SignerError::Locked));
            None
        }
        None => Some(v.scan_key(&wallet(1), &scan[0]).unwrap()),
    };
    if let Some(key) = key {
        assert_eq!(key.master_key().unwrap().depth, 0, "{state:?}");
    }
    for purpose in [platform_op(0, 0), platform_op(u64::MAX, u64::MAX)] {
        assert_eq!(
            v.scan_key(&wallet(1), &token(v, purpose, credential))
                .map(drop),
            Err(VaultError::GrantPurposeMismatch),
            "{state:?} {purpose:?}"
        );
    }
}

/// An `IdentityScan` grant is no flow grant: it signs nothing.
#[test]
fn an_identity_scan_token_issues_no_signer() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let t = token(&v, GrantPurpose::IdentityScan, Credential::None);
    assert_eq!(
        v.signer(&wallet(1), &t).map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );
    for scope in [
        SignerScope::PlatformIdentity,
        SignerScope::DashPayCrypto,
        SignerScope::PlatformFunding { max_duffs: 0 },
    ] {
        assert_eq!(
            v.platform_signer(&wallet(1), &t, scope).map(drop),
            Err(VaultError::GrantPurposeMismatch)
        );
    }
}

// MARK: authorize_set (§3.3)

/// One passphrase check for the whole set: a wrong passphrase counts once
/// in the throttle, and a right one issues one grant per purpose, in order,
/// with the same wallet and lifetime.
#[test]
fn authorize_set_checks_the_passphrase_once() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.lock();
    let set = [
        platform_op(0, 7_000),
        GrantPurpose::Spend { max_duffs: 1_234 },
        GrantPurpose::SignMessage,
    ];
    let w = Some(&wallet(1));

    for n in 1..=2 {
        assert!(matches!(
            v.authorize_set(&set, w, Credential::Passphrase(OTHER)),
            Err(VaultError::WrongPassphrase { failed_attempts, .. }) if failed_attempts == n
        ));
        assert_eq!(v.status().failed_attempts, n);
    }

    let grants = v
        .authorize_set(&set, w, Credential::Passphrase(PASS))
        .unwrap();
    assert_eq!(v.status().failed_attempts, 0);
    assert_eq!(
        grants.iter().map(|g| g.purpose).collect::<Vec<_>>(),
        set.to_vec()
    );
    for g in &grants {
        assert_eq!(g.wallet, Some(wallet(1)));
        assert_eq!(g.expires_at, T0 + TTL);
        assert!(g.single_use);
    }
    let ids: std::collections::HashSet<_> = grants.iter().map(|g| &g.id).collect();
    assert_eq!(ids.len(), 3);
    // The lock state is unchanged; each grant carries its own key.
    assert_eq!(v.lock_state(), LockState::Locked);
    let mut tokens = redeem_all(&v, &grants);
    assert!(tokens.iter().all(GrantToken::has_own_key));
    let hold = v.hold_key(&mut tokens).unwrap().unwrap();
    v.platform_signer_held(&wallet(1), &hold, &tokens[0], SignerScope::PlatformIdentity)
        .unwrap();
    v.signer_held(&wallet(1), &hold, &tokens[1]).unwrap();
    v.signer_held(&wallet(1), &hold, &tokens[2]).unwrap();
}

/// All or nothing: a set with one purpose the credential does not allow
/// issues no grant, and the binding rules apply to every purpose.
#[test]
fn authorize_set_is_all_or_nothing() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let w = Some(&wallet(1));

    // Unlocked, no credential: reveal needs the passphrase.
    assert_eq!(
        v.authorize_set(
            &[
                GrantPurpose::Spend { max_duffs: 1 },
                GrantPurpose::RevealSecret
            ],
            w,
            Credential::None
        ),
        Err(VaultError::CredentialRequired)
    );
    // A wallet-bound purpose without a wallet, or `ChangeCredential` with one.
    assert!(matches!(
        v.authorize_set(
            &[GrantPurpose::SignMessage, GrantPurpose::ChangeCredential],
            w,
            Credential::None
        ),
        Err(VaultError::InvalidArgument(_))
    ));
    assert!(matches!(
        v.authorize_set(
            &[GrantPurpose::ChangeCredential, GrantPurpose::SignMessage],
            None,
            Credential::Passphrase(PASS)
        ),
        Err(VaultError::InvalidArgument(_))
    ));
    assert!(matches!(
        v.authorize_set(&[], w, Credential::None),
        Err(VaultError::InvalidArgument(_))
    ));
    // Locked, no credential.
    v.lock();
    assert_eq!(
        v.authorize_set(
            &[platform_op(1, 1), GrantPurpose::Spend { max_duffs: 1 }],
            w,
            Credential::None
        ),
        Err(VaultError::Locked)
    );
    // A one-element set is `authorize`.
    let g = v
        .authorize_set(
            &[GrantPurpose::IdentityScan],
            w,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].purpose, GrantPurpose::IdentityScan);
}

// MARK: QuickUnlock for PlatformOp (§3.7, DEC-67)

/// Encrypted vault with wallet 1, quick unlock enrolled (default limit),
/// locked; the passphrase was entered at `T0`.
fn enrolled(fx: &Fixture) -> (Vault, Vec<u8>) {
    let v = encrypted(fx);
    let grant = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    let key = v.enroll_quick_unlock(&grant.id).unwrap();
    v.lock();
    (v, key.to_vec())
}

#[test]
fn quick_unlock_issues_platform_op_within_the_combined_limit() {
    const L: u64 = DEFAULT_QUICK_UNLOCK_SPEND_LIMIT;
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    let w = Some(&wallet(1));
    let exceeded = Err(VaultError::QuickUnlockLimitExceeded { limit_duffs: L });
    let qu = |purposes: &[GrantPurpose]| {
        v.authorize_set(purposes, w, Credential::QuickUnlock(&key))
            .map(|g| g.len())
    };
    assert_eq!(CREDITS_PER_DUFF, 1_000);

    // Within the limit, and one duff (or one credit past a duff) above it.
    for (ok, over) in [
        (platform_op(L, 0), platform_op(L + 1, 0)),
        (
            platform_op(0, L * CREDITS_PER_DUFF),
            platform_op(0, L * CREDITS_PER_DUFF + 1),
        ),
        (platform_op(L - 1, 1_000), platform_op(L - 1, 1_001)),
        (platform_op(L - 1, 1), platform_op(L, 1)),
    ] {
        assert_eq!(qu(&[ok]), Ok(1), "{ok:?}");
        assert_eq!(qu(&[over]), exceeded, "{over:?}");
    }
    // A value past u64 is over any limit, not a panic.
    assert_eq!(qu(&[platform_op(u64::MAX, u64::MAX)]), exceeded);
    assert_eq!(
        qu(&[
            platform_op(u64::MAX, 0),
            GrantPurpose::Spend { max_duffs: 1 }
        ]),
        exceeded
    );

    // "Accept and pay" at the limit: one sum over the set, `Spend` included.
    let accept_credits = 2_500_500; // 2501 duffs, rounded up
    let at_limit = [
        platform_op(0, accept_credits),
        GrantPurpose::Spend {
            max_duffs: L - 2_501,
        },
    ];
    assert_eq!(qu(&at_limit), Ok(2));
    let one_more = [
        at_limit[0],
        GrantPurpose::Spend {
            max_duffs: L - 2_500,
        },
    ];
    assert_eq!(qu(&one_more), exceeded);
    // Above the limit the passphrase is asked for, and it issues the set.
    assert_eq!(
        v.authorize_set(&one_more, w, Credential::Passphrase(PASS))
            .map(|g| g.len()),
        Ok(2)
    );

    // The grant carries its own key on a locked vault and yields signers.
    let grants = v
        .authorize_set(&at_limit, w, Credential::QuickUnlock(&key))
        .unwrap();
    let mut tokens = redeem_all(&v, &grants);
    assert_eq!(v.lock_state(), LockState::Locked);
    let hold = v.hold_key(&mut tokens).unwrap().unwrap();
    v.platform_signer_held(&wallet(1), &hold, &tokens[0], SignerScope::PlatformIdentity)
        .unwrap();
    v.signer_held(&wallet(1), &hold, &tokens[1]).unwrap();

    // Each `PlatformOp` grant rounds its credits up on its own (§3.7's
    // per-grant value), so two half-duff credit caps count two duffs.
    assert_eq!(qu(&[platform_op(L - 2, 500), platform_op(0, 500)]), Ok(2));
    assert_eq!(
        qu(&[platform_op(L - 1, 500), platform_op(0, 500)]),
        exceeded
    );

    // The identity scan never comes from quick unlock, alone or in a set.
    for set in [
        &[GrantPurpose::IdentityScan][..],
        &[platform_op(0, 1), GrantPurpose::IdentityScan],
    ] {
        assert_eq!(qu(set), Err(VaultError::CredentialRequired));
    }
}

#[test]
fn quick_unlock_platform_op_needs_a_fresh_passphrase() {
    let fx = Fixture::new();
    let (v, key) = enrolled(&fx);
    let w = Some(&wallet(1));
    fx.clock.advance(PASSPHRASE_MAX_AGE_SECS);
    v.authorize(platform_op(1, 1), w, Credential::QuickUnlock(&key))
        .unwrap();
    fx.clock.advance(1);
    for set in [
        &[platform_op(1, 1)][..],
        &[platform_op(0, 1), GrantPurpose::Spend { max_duffs: 1 }],
    ] {
        assert_eq!(
            v.authorize_set(set, w, Credential::QuickUnlock(&key)),
            Err(VaultError::PassphraseStale)
        );
    }
    // Entering the passphrase makes it fresh again.
    v.authorize(platform_op(1, 1), w, Credential::Passphrase(PASS))
        .unwrap();
    v.authorize(platform_op(1, 1), w, Credential::QuickUnlock(&key))
        .unwrap();
}

// MARK: KeyHold (§3.5)

#[tokio::test]
async fn dropping_the_key_hold_stops_every_signer_of_the_set() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.lock();
    // "Accept and pay": one prompt, two grants, one hold.
    let grants = v
        .authorize_set(
            &[platform_op(0, 1_000), GrantPurpose::Spend { max_duffs: 1 }],
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    let mut tokens = redeem_all(&v, &grants);
    let hold = v.hold_key(&mut tokens).unwrap().expect("own-key tokens");
    assert!(tokens.iter().all(|t| hold.holds(t) && t.has_own_key()));

    let identity = v
        .platform_signer_held(&wallet(1), &hold, &tokens[0], SignerScope::PlatformIdentity)
        .unwrap();
    let spend = v.signer_held(&wallet(1), &hold, &tokens[1]).unwrap();
    // The caps still apply to a held token.
    assert_eq!(
        v.platform_signer_held(
            &wallet(1),
            &hold,
            &tokens[0],
            SignerScope::PlatformFunding { max_duffs: 0 }
        )
        .map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );
    let clones = [identity.clone(), identity.clone()];
    let spend_clone = spend.clone();
    let key = path(IDENTITY_KEY);
    for s in clones.iter().chain([&identity]) {
        s.public_key(&key).await.unwrap();
    }
    spend_clone.sign_message(&path(BIP44), b"m").await.unwrap();

    let epoch = v.epoch();
    drop(hold);
    // Every clone, of both grants' signers, fails `Locked`, while the vault
    // itself is untouched (still locked, same epoch).
    for s in clones.iter().chain([&identity]) {
        assert_eq!(s.public_key(&key).await, Err(SignerError::Locked));
    }
    assert_eq!(
        spend_clone.sign_message(&path(BIP44), b"m").await,
        Err(SignerError::Locked)
    );
    assert_eq!(
        spend.sign_ecdsa(&path(BIP44), [1; 32]).await.map(drop),
        Err(SignerError::Locked)
    );
    // The tokens' copies were erased, and a held token issues only through
    // its hold, now gone: nothing more comes from them.
    assert!(matches!(
        v.platform_signer(&wallet(1), &tokens[0], SignerScope::PlatformIdentity),
        Err(VaultError::InvalidArgument(_))
    ));
    assert!(matches!(
        v.signer(&wallet(1), &tokens[1]),
        Err(VaultError::InvalidArgument(_))
    ));
    assert_eq!(v.epoch(), epoch);
}

#[test]
fn a_hold_needs_own_keys_and_serves_only_its_tokens() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    // Unlocked with scope Full: the tokens use the vault's key; no hold.
    let mut vault_key = vec![token(&v, platform_op(1, 1), Credential::None)];
    assert!(!vault_key[0].has_own_key());
    assert!(v.hold_key(&mut vault_key).unwrap().is_none());
    assert!(v.hold_key(&mut []).unwrap().is_none());

    v.lock();
    let mut a = vec![token(&v, platform_op(1, 1), Credential::Passphrase(PASS))];
    let mut b = vec![token(&v, platform_op(1, 1), Credential::Passphrase(PASS))];
    let hold_a = v.hold_key(&mut a).unwrap().unwrap();
    let hold_b = v.hold_key(&mut b).unwrap().unwrap();
    for (hold, token) in [(&hold_a, &b[0]), (&hold_b, &a[0]), (&hold_a, &vault_key[0])] {
        assert!(matches!(
            v.platform_signer_held(&wallet(1), hold, token, SignerScope::DashPayCrypto),
            Err(VaultError::InvalidArgument(_))
        ));
    }
    let signer = v
        .platform_signer_held(&wallet(1), &hold_a, &a[0], SignerScope::DashPayCrypto)
        .unwrap();
    // An epoch change ends a held signer as it ends every signer, hold or
    // not, and a token of an ended epoch cannot be held.
    let mut stale = vec![token(&v, platform_op(1, 1), Credential::Passphrase(PASS))];
    v.unlock(PASS, UnlockScope::Full).unwrap();
    assert_eq!(
        signer.contact_info_seal(&path(IDENTITY_KEY), 0, &[0; 32], b"", &[0; 16]),
        Err(SignerError::Locked)
    );
    assert_eq!(v.hold_key(&mut stale).map(drop), Err(VaultError::Locked));
}

/// Review DW-E0-04-P1 r1 ruling: a hold covers every token it is given or
/// fails, changing none. Refused: a set mixing vault-key and own-key
/// tokens, a token already held, and a token that issued a signer (whose
/// copy no hold could erase; only the M1 purposes issue one directly).
#[test]
fn a_hold_covers_the_whole_set_or_nothing() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let invalid = |r: Result<Option<dw_vault::KeyHold>, VaultError>| {
        assert!(matches!(r, Err(VaultError::InvalidArgument(_))), "{r:?}");
    };
    let vault_key = token(&v, platform_op(1, 1), Credential::None);
    v.lock();
    let own = |v: &Vault| token(v, platform_op(1, 1), Credential::Passphrase(PASS));
    let spend = token(
        &v,
        GrantPurpose::Spend { max_duffs: 1 },
        Credential::Passphrase(PASS),
    );

    // Mixed vault-key and own-key tokens (the vault-key one also of an
    // ended epoch: the mix is refused first).
    let mut mixed = [own(&v), vault_key];
    invalid(v.hold_key(&mut mixed));
    assert!(!format!("{:?}", mixed[0]).contains("held"));

    // A token that issued a signer, in a set with one that did not.
    let direct = v.signer(&wallet(1), &spend).unwrap();
    let mut with_signer = [own(&v), spend];
    invalid(v.hold_key(&mut with_signer));
    assert!(
        with_signer
            .iter()
            .all(|t| format!("{t:?}").contains("\"own\""))
    );
    drop(direct);
    // Once that signer is gone, the same set is held whole.
    let hold = v.hold_key(&mut with_signer).unwrap().unwrap();
    assert!(with_signer.iter().all(|t| hold.holds(t)));

    // A token already held, alone or with a fresh one.
    invalid(v.hold_key(&mut with_signer));
    let mut again = [own(&v)];
    let other = v.hold_key(&mut again).unwrap().unwrap();
    let [held] = again;
    let mut with_held = [own(&v), held];
    invalid(v.hold_key(&mut with_held));
    assert!(format!("{:?}", with_held[0]).contains("\"own\""));
    assert!(other.holds(&with_held[1]));
    // A fresh token with one of an ended epoch.
    let stale = own(&v);
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    let mut with_stale = [own(&v), stale];
    assert_eq!(
        v.hold_key(&mut with_stale).map(drop),
        Err(VaultError::Locked)
    );
    assert!(
        with_stale
            .iter()
            .all(|t| format!("{t:?}").contains("\"own\""))
    );
}

/// Review DW-E0-04-P1 r1 ruling: an own-key `PlatformOp` or `IdentityScan`
/// token issues signers only through a hold; directly it is refused. A
/// held token of any purpose issues only through its hold. Only the M1
/// purposes (`Spend`, `SignMessage`) issue a direct own-key signer.
#[tokio::test]
async fn own_key_platform_and_scan_tokens_issue_only_through_a_hold() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.lock();
    let pass = Credential::Passphrase(PASS);
    let invalid = |r: Result<(), VaultError>, what: &str| {
        assert!(
            matches!(r, Err(VaultError::InvalidArgument(_))),
            "{what}: {r:?}"
        );
    };

    let platform = token(&v, platform_op(1, 1), pass);
    for scope in [
        SignerScope::PlatformIdentity,
        SignerScope::DashPayCrypto,
        SignerScope::PlatformFunding { max_duffs: 1 },
    ] {
        invalid(
            v.platform_signer(&wallet(1), &platform, scope).map(drop),
            "direct Platform signer",
        );
    }
    let scan = token(&v, GrantPurpose::IdentityScan, pass);
    invalid(v.scan_key(&wallet(1), &scan).map(drop), "direct scan key");

    // The M1 exception: a direct own-key signer for Spend and SignMessage.
    for purpose in [
        GrantPurpose::Spend { max_duffs: 1 },
        GrantPurpose::SignMessage,
    ] {
        let t = token(&v, purpose, pass);
        v.signer(&wallet(1), &t)
            .unwrap()
            .sign_message(&path(BIP44), b"m")
            .await
            .unwrap();
    }

    // Held, every purpose issues through its hold and nowhere else.
    let mut set = [
        platform,
        scan,
        token(&v, GrantPurpose::Spend { max_duffs: 1 }, pass),
    ];
    let hold = v.hold_key(&mut set).unwrap().unwrap();
    invalid(
        v.platform_signer(&wallet(1), &set[0], SignerScope::DashPayCrypto)
            .map(drop),
        "held Platform token, directly",
    );
    invalid(
        v.scan_key(&wallet(1), &set[1]).map(drop),
        "held scan token, directly",
    );
    invalid(
        v.signer(&wallet(1), &set[2]).map(drop),
        "held Spend token, directly",
    );
    v.platform_signer_held(&wallet(1), &hold, &set[0], SignerScope::DashPayCrypto)
        .unwrap();
    v.scan_key_held(&wallet(1), &hold, &set[1])
        .unwrap()
        .master_key()
        .unwrap();
    v.signer_held(&wallet(1), &hold, &set[2]).unwrap();

    // Vault-key tokens (unlocked) still issue directly.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let t = token(&v, platform_op(1, 1), Credential::None);
    v.platform_signer(&wallet(1), &t, SignerScope::PlatformIdentity)
        .unwrap();
    let t = token(&v, GrantPurpose::IdentityScan, Credential::None);
    v.scan_key(&wallet(1), &t).unwrap();
}

/// The redacted `Debug` of a hold and a held token.
#[test]
fn a_hold_does_not_print_its_key() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.lock();
    let mut t = vec![token(&v, platform_op(1, 1), Credential::Passphrase(PASS))];
    assert!(format!("{:?}", t[0]).contains("key: \"own\""));
    let hold = v.hold_key(&mut t).unwrap().unwrap();
    assert_eq!(format!("{hold:?}"), "KeyHold(<redacted>)");
    assert!(format!("{:?}", t[0]).contains("key: \"held\""));
}

// MARK: epoch (§3.5, §8.6)

#[test]
fn the_epoch_moves_on_every_epoch_change_and_nothing_else() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let mut last = v.epoch();
    let mut moved = |v: &Vault, what: &str| {
        let now = v.epoch();
        assert!(now > last, "{what}: {last} → {now}");
        last = now;
    };
    v.lock();
    moved(&v, "lock");
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    moved(&v, "unlock for mixing");
    v.unlock(PASS, UnlockScope::Full).unwrap();
    moved(&v, "scope change");
    v.change_passphrase(PASS, OTHER).unwrap();
    moved(&v, "passphrase change");
    assert_eq!(v.lock_state(), LockState::Unlocked);

    // Grants, redemptions, signers and reads leave it alone.
    let e = v.epoch();
    let mut ts = vec![token(&v, platform_op(1, 1), Credential::Passphrase(OTHER))];
    assert!(v.hold_key(&mut ts).unwrap().is_none());
    v.platform_signer(&wallet(1), &ts[0], SignerScope::DashPayCrypto)
        .unwrap();
    v.status();
    v.unlock(OTHER, UnlockScope::Full).unwrap();
    assert_eq!(v.epoch(), e, "an unlock that changes nothing");
    v.lock();
    moved(&v, "lock again");
}
