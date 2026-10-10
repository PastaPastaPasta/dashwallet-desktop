//! Platform signer scopes (DASHPAY §2.6/§3.3, roadmap E0-03): which grant
//! and lock state issue them, and that each refuses every path and every
//! use outside it.

mod common;

use std::str::FromStr;

use common::*;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dashcore::signer::double_sha;
use dw_vault::{
    Credential, GrantKind, GrantPurpose, GrantToken, SignerError, SignerScope, UnlockScope, Vault,
    VaultError, VaultSigner, WalletSigner,
};
use key_wallet::bip32::{ChildNumber, DerivationPath};
use key_wallet::{ExtendedPubKeySigner, Network, Signer};

const FUNDING: SignerScope = SignerScope::PlatformFunding { max_duffs: 100_000 };
/// A `PlatformOp` grant that covers [`FUNDING`] and identity signatures.
const PLATFORM_OP: GrantPurpose = GrantPurpose::PlatformOp {
    max_duffs: 100_000,
    max_credits: 1_000_000,
};

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

/// A DIP-15 receiving account (`m/9'/coin'/15'/0'/<user>/<friend>`) plus
/// `extra` steps.
fn receiving(coin: u32, extra: &[ChildNumber]) -> DerivationPath {
    let mut p = vec![
        ChildNumber::Hardened { index: 9 },
        ChildNumber::Hardened { index: coin },
        ChildNumber::Hardened { index: 15 },
        ChildNumber::Hardened { index: 0 },
        ChildNumber::Normal256 { index: [0x11; 32] },
        ChildNumber::Normal256 { index: [0x22; 32] },
    ];
    p.extend_from_slice(extra);
    p.into()
}

/// `base` with its step `at` replaced (negative: from the end).
fn with_step(base: DerivationPath, at: isize, step: ChildNumber) -> DerivationPath {
    let mut p: Vec<ChildNumber> = base.into();
    let i = if at < 0 { p.len() as isize + at } else { at } as usize;
    p[i] = step;
    p.into()
}

/// `v` created unencrypted (no KDF runs), holding wallet 1.
fn unencrypted(v: Vault) -> Vault {
    v.create(None).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

fn unencrypted_vault(fx: &Fixture) -> Vault {
    unencrypted(fx.open())
}

/// An unencrypted mainnet vault (coin type 5).
fn mainnet_vault(fx: &Fixture) -> Vault {
    unencrypted(Vault::open(fx.vault_dir(), Network::Mainnet, "mainnet", fx.config()).unwrap())
}

/// A redeemed `PlatformOp` grant for wallet 1.
fn platform_token(v: &Vault) -> GrantToken {
    let grant = v
        .authorize(PLATFORM_OP, Some(&wallet(1)), Credential::None)
        .unwrap();
    v.redeem_grant(&grant.id, GrantKind::PlatformOp, Some(&wallet(1)))
        .unwrap()
}

/// A redeemed `IdentityScan` grant for wallet 1.
fn scan_token(v: &Vault) -> GrantToken {
    let grant = v
        .authorize(
            GrantPurpose::IdentityScan,
            Some(&wallet(1)),
            Credential::None,
        )
        .unwrap();
    v.redeem_grant(&grant.id, GrantKind::IdentityScan, Some(&wallet(1)))
        .unwrap()
}

fn platform_signer(v: &Vault, scope: SignerScope) -> VaultSigner {
    v.platform_signer(&wallet(1), &platform_token(v), scope)
        .unwrap()
}

fn full_signer(v: &Vault) -> VaultSigner {
    let grant = v
        .authorize(
            GrantPurpose::Spend { max_duffs: 1 },
            Some(&wallet(1)),
            Credential::None,
        )
        .unwrap();
    let token = v
        .redeem_grant(&grant.id, GrantKind::Spend, Some(&wallet(1)))
        .unwrap();
    v.signer(&wallet(1), &token).unwrap()
}

fn refused<T: std::fmt::Debug>(r: Result<T, SignerError>) -> bool {
    matches!(r, Err(SignerError::PathNotAllowed(_)))
}

/// Every operation a signer offers, at one path: `true` allowed, `false`
/// refused as out of scope. Operations that need an identity
/// key root (`contactInfo`) use `path` as the root.
#[derive(Debug, PartialEq, Eq)]
struct Uses {
    sign: bool,
    message: bool,
    public: bool,
    xpub: bool,
    identity_sign: bool,
    ecdh: bool,
    account_reference: bool,
    contact_info: bool,
    export: bool,
}

const NONE: Uses = Uses {
    sign: false,
    message: false,
    public: false,
    xpub: false,
    identity_sign: false,
    ecdh: false,
    account_reference: false,
    contact_info: false,
    export: false,
};

async fn uses(s: &VaultSigner, p: &DerivationPath) -> Uses {
    let peer = PublicKey::from_secret_key(&SecretKey::from_secret_bytes([0x42; 32]).unwrap());
    // A key-data mismatch is not a scope refusal: pass the real key when the
    // scope lets us read it, garbage otherwise.
    let key_data = s
        .public_key(p)
        .await
        .map(|k| k.serialize().to_vec())
        .unwrap_or_default();
    Uses {
        sign: !refused(s.sign_ecdsa(p, [1; 32]).await),
        message: !refused(s.sign_message(p, b"m").await),
        public: !refused(s.public_key(p).await),
        xpub: !refused(s.extended_public_key(p).await),
        identity_sign: !refused(s.sign_identity(p, &key_data, b"d")),
        ecdh: !refused(s.ecdh_shared_secret(p, &peer)),
        account_reference: !refused(s.account_reference(p, &[0; 69], 0, 0))
            && !refused(s.unmask_account_reference(p, &[0; 69], 0)),
        contact_info: !refused(s.contact_info_seal(p, 0, &[3; 32], b"x", &[1; 16]))
            && !refused(s.contact_info_open(p, 0, &[3; 32], &[0; 32])),
        export: !refused(s.export_auto_accept_key(p)),
    }
}

/// Paths of every shape the scopes are made of, plus near misses, for the
/// network of coin type `coin`; `other` is the other network's coin type.
fn all_paths(coin: u32, other: u32) -> Vec<(&'static str, DerivationPath)> {
    let c = |s: &str| {
        path(
            &s.replace("{c}", &coin.to_string())
                .replace("{o}", &other.to_string()),
        )
    };
    let big = ChildNumber::Normal256 { index: [0x33; 32] };
    let big_hardened = ChildNumber::Hardened256 { index: [0x33; 32] };
    // Raw 31-bit variants out of range (review DW-E0-03 M2): `Hardened` with
    // the top bit set aliases the key of the low bits; `Normal` at 2^31 or
    // above derives an off-shape key.
    let normal = |index| ChildNumber::Normal { index };
    let hardened = |index| ChildNumber::Hardened { index };
    let top = 1u32 << 31;
    vec![
        ("master", DerivationPath::master()),
        ("bip44 address", c("m/44'/{c}'/0'/0/3")),
        ("bip44 account 0", c("m/44'/{c}'/0'")),
        ("bip44 account 1", c("m/44'/{c}'/1'")),
        ("bip44 other-network address", c("m/44'/{o}'/0'/0/3")),
        ("bip44 other-network account 0", c("m/44'/{o}'/0'")),
        (
            "bip44 address 256-bit leaf",
            with_step(c("m/44'/{c}'/0'/0/3"), -1, big),
        ),
        (
            "bip44 address leaf 2^31",
            with_step(c("m/44'/{c}'/0'/0/3"), -1, normal(top)),
        ),
        (
            "bip44 account 256-bit, address",
            with_step(c("m/44'/{c}'/0'/0/3"), 2, big_hardened),
        ),
        (
            "bip44 account 2^31, address",
            with_step(c("m/44'/{c}'/0'/0/3"), 2, hardened(top)),
        ),
        (
            "bip44 account u32::MAX, address",
            with_step(c("m/44'/{c}'/0'/0/3"), 2, hardened(u32::MAX)),
        ),
        ("bip32 address", c("m/0'/1/3")),
        (
            "bip32 address leaf u32::MAX",
            with_step(path("m/0'/1/3"), -1, normal(u32::MAX)),
        ),
        // A BIP32 account numbered 44' is a BIP32 account, not BIP44.
        ("bip32 address of account 44'", c("m/44'/0/0")),
        ("coinjoin address", c("m/9'/{c}'/4'/0'/0/1")),
        ("identity key", c("m/9'/{c}'/5'/0'/0'/1'/4'")),
        (
            "identity key id 2^31|4",
            with_step(c("m/9'/{c}'/5'/0'/0'/1'/4'"), -1, hardened(top | 4)),
        ),
        (
            "identity index u32::MAX",
            with_step(c("m/9'/{c}'/5'/0'/0'/1'/4'"), 5, hardened(u32::MAX)),
        ),
        ("identity root", c("m/9'/{c}'/5'/0'/0'/1'")),
        ("identity key other network", c("m/9'/{o}'/5'/0'/0'/1'/4'")),
        ("identity key bls", c("m/9'/{c}'/5'/0'/1'/1'/4'")),
        ("identity key child", c("m/9'/{c}'/5'/0'/0'/1'/4'/0'")),
        (
            "identity key 256-bit index",
            with_step(c("m/9'/{c}'/5'/0'/0'/1'/4'"), -1, big_hardened),
        ),
        (
            "identity key, feature out of range",
            with_step(
                c("m/9'/{c}'/5'/0'/0'/1'/4'"),
                2,
                ChildNumber::Hardened { index: 5 | 1 << 31 },
            ),
        ),
        ("contactInfo key", c("m/9'/{c}'/5'/0'/0'/1'/4'/65536'/0'")),
        (
            "other identity-key child",
            c("m/9'/{c}'/5'/0'/0'/1'/4'/65538'/0'"),
        ),
        ("registration credit key", c("m/9'/{c}'/5'/1'/0")),
        (
            "registration credit key leaf 2^31",
            with_step(c("m/9'/{c}'/5'/1'/0"), -1, normal(top)),
        ),
        ("registration credit key, hardened", c("m/9'/{c}'/5'/1'/4'")),
        ("top-up credit key", c("m/9'/{c}'/5'/2'/0'/1")),
        ("top-up account", c("m/9'/{c}'/5'/2'/0'")),
        (
            "top-up account u32::MAX",
            with_step(c("m/9'/{c}'/5'/2'/0'"), -1, hardened(u32::MAX)),
        ),
        ("invitation credit key", c("m/9'/{c}'/5'/3'/2")),
        ("address top-up credit key", c("m/9'/{c}'/5'/4'/2")),
        ("receiving account", receiving(coin, &[])),
        ("receiving account other network", receiving(other, &[])),
        (
            "receiving address",
            receiving(coin, &[ChildNumber::Normal { index: 0 }]),
        ),
        ("receiving address 256-bit leaf", receiving(coin, &[big])),
        (
            "receiving address leaf 2^31",
            receiving(coin, &[normal(top)]),
        ),
        (
            "receiving account, hardened friend",
            with_step(receiving(coin, &[]), 5, big_hardened),
        ),
        ("auto-accept key", c("m/9'/{c}'/16'/1900000000'")),
        (
            "auto-accept expiry 2^31|1",
            with_step(c("m/9'/{c}'/16'/1'"), -1, hardened(top | 1)),
        ),
        ("auto-accept unhardened", c("m/9'/{c}'/16'/1900000000")),
        ("auto-accept other network", c("m/9'/{o}'/16'/1900000000'")),
        ("dashpay root", c("m/9'/{c}'/15'")),
        ("platform payment", c("m/9'/{c}'/17'/0'/0'/0")),
        ("provider owner key", c("m/9'/{c}'/3'/2'/0")),
    ]
}

/// The expected uses of `scope` at the path named `name`.
fn expected(scope: SignerScope, name: &str) -> Uses {
    let sign_public_key = Uses {
        sign: true,
        public: true,
        ..NONE
    };
    let public = Uses {
        public: true,
        xpub: true,
        ..NONE
    };
    match (scope, name) {
        (SignerScope::PlatformIdentity, "identity key") => Uses {
            identity_sign: true,
            ..sign_public_key
        },
        (
            SignerScope::PlatformFunding { .. },
            "bip44 address"
            | "bip32 address"
            | "bip32 address of account 44'"
            | "registration credit key"
            | "top-up credit key"
            | "invitation credit key"
            | "receiving address",
        ) => sign_public_key,
        (SignerScope::PlatformFunding { .. }, "top-up account") => Uses { xpub: true, ..NONE },
        (SignerScope::DashPayCrypto, "bip44 account 0" | "receiving account") => public,
        (SignerScope::DashPayCrypto, "auto-accept key") => Uses {
            export: true,
            ..public
        },
        (SignerScope::DashPayCrypto, "identity key") => Uses {
            ecdh: true,
            account_reference: true,
            // As a root: the contactInfo keys are its 65536'/65537' children.
            contact_info: true,
            ..NONE
        },
        _ => NONE,
    }
}

async fn check_platform_matrix(v: &Vault, coin: u32, other: u32) {
    for scope in [
        SignerScope::PlatformIdentity,
        SignerScope::DashPayCrypto,
        FUNDING,
    ] {
        let s = platform_signer(v, scope);
        for (name, p) in all_paths(coin, other) {
            assert_eq!(
                uses(&s, &p).await,
                expected(scope, name),
                "{scope:?} at {name} ({p})"
            );
        }
    }
}

#[tokio::test]
async fn every_platform_scope_refuses_every_path_and_use_outside_it() {
    let fx = Fixture::new();
    check_platform_matrix(&unencrypted_vault(&fx), 1, 5).await;
}

#[tokio::test]
async fn on_mainnet_the_scopes_take_coin_type_5_only() {
    let fx = Fixture::new();
    check_platform_matrix(&mainnet_vault(&fx), 5, 1).await;
}

#[tokio::test]
async fn operation_shapes_hold_even_for_a_full_signer() {
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    let full = full_signer(&v);
    for (name, p) in all_paths(1, 5) {
        let identity = name == "identity key";
        let want = Uses {
            sign: true,
            message: true,
            public: true,
            xpub: true,
            identity_sign: identity,
            ecdh: identity,
            account_reference: identity,
            contact_info: identity,
            export: name == "auto-accept key",
        };
        // A path whose derivation fails (a 256-bit hardened step) is a
        // derivation error, not a refusal, for a Full signer.
        assert_eq!(uses(&full, &p).await, want, "Full at {name} ({p})");
    }
}

#[tokio::test]
async fn coinjoin_scopes_get_no_platform_operation() {
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    let mixing = v.mixing_signer(&wallet(1)).unwrap();
    let funding = v.mixing_funding_signer(&wallet(1)).unwrap();
    let mixing_uses = |on: bool| Uses {
        sign: on,
        message: on,
        public: on,
        xpub: on,
        ..NONE
    };
    for (name, p) in all_paths(1, 5) {
        let coinjoin = name == "coinjoin address";
        let bip44 = name.starts_with("bip44 address");
        assert_eq!(uses(&mixing, &p).await, mixing_uses(coinjoin), "{name}");
        // CoinJoinFunding keeps its old rule, `m/44'/coin'/account'/…` with
        // four or more steps.
        assert_eq!(
            uses(&funding, &p).await,
            mixing_uses(coinjoin || bip44),
            "funding {name}"
        );
    }
}

#[tokio::test]
async fn platform_grants_issue_scoped_signers_only() {
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    let token = platform_token(&v);
    // No full signer from a Platform grant any more.
    assert_eq!(
        v.signer(&wallet(1), &token).unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    for scope in [
        SignerScope::Full,
        SignerScope::CoinJoinOnly,
        SignerScope::CoinJoinFunding,
    ] {
        assert!(matches!(
            v.platform_signer(&wallet(1), &token, scope).unwrap_err(),
            VaultError::InvalidArgument(_)
        ));
    }
    // One token issues every Platform scope a flow needs, for its wallet only.
    for scope in [
        SignerScope::PlatformIdentity,
        SignerScope::DashPayCrypto,
        FUNDING,
    ] {
        assert_eq!(
            v.platform_signer(&wallet(1), &token, scope)
                .unwrap()
                .scope(),
            scope
        );
        assert_eq!(
            v.platform_signer(&wallet(2), &token, scope).unwrap_err(),
            VaultError::GrantPurposeMismatch
        );
    }
    assert_eq!(FUNDING.max_duffs(), Some(100_000));

    // Other grants get no Platform signer.
    for purpose in [
        GrantPurpose::Spend { max_duffs: 1 },
        GrantPurpose::SignMessage,
    ] {
        let grant = v
            .authorize(purpose, Some(&wallet(1)), Credential::None)
            .unwrap();
        let token = v
            .redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
            .unwrap();
        assert_eq!(
            v.platform_signer(&wallet(1), &token, SignerScope::PlatformIdentity)
                .unwrap_err(),
            VaultError::GrantPurposeMismatch
        );
    }
}

#[tokio::test]
async fn background_crypto_needs_a_prompt_free_full_key() {
    let fx = Fixture::new();
    let v = fx.open();
    assert_eq!(
        v.dashpay_crypto_signer(&wallet(1)).unwrap_err(),
        VaultError::NoVault
    );
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();

    // Unlocked: issued; the background signer still signs nothing.
    let crypto = v.dashpay_crypto_signer(&wallet(1)).unwrap();
    assert_eq!(crypto.scope(), SignerScope::DashPayCrypto);
    assert!(refused(
        crypto.sign_ecdsa(&path("m/44'/1'/0'/0/0"), [1; 32]).await
    ));
    assert!(refused(
        crypto
            .sign_ecdsa(&path("m/9'/1'/5'/0'/0'/0'/0'"), [1; 32])
            .await
    ));
    assert_eq!(
        v.dashpay_crypto_signer(&wallet(9)).unwrap_err(),
        VaultError::NoSecret
    );

    // Lock: it stops, and is not issued.
    v.lock();
    assert_eq!(
        crypto
            .extended_public_key(&path("m/44'/1'/0'"))
            .await
            .unwrap_err(),
        SignerError::Locked
    );
    assert_eq!(
        v.dashpay_crypto_signer(&wallet(1)).unwrap_err(),
        VaultError::Locked
    );

    // Mixing-only unlock: not issued.
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    assert_eq!(
        v.dashpay_crypto_signer(&wallet(1)).unwrap_err(),
        VaultError::MixingOnly
    );

    // Full unlock after mixing-only: issued again.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    v.dashpay_crypto_signer(&wallet(1)).unwrap();

    // An unencrypted vault: issued without a prompt.
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    v.dashpay_crypto_signer(&wallet(1)).unwrap();
}

/// Review DW-E0-03 M1: the master key leaves only under a redeemed
/// `IdentityScan` grant for its wallet (E0-04 design §3.2); a grant-less
/// caller, or one with a capped `PlatformOp` flow grant, has no way to it.
#[tokio::test]
async fn the_scan_key_needs_an_identity_scan_grant_for_its_wallet() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();

    // Unlocked: the bring-up authorizes the grant itself, without a prompt.
    let scan = v.scan_key(&wallet(1), &scan_token(&v)).unwrap();
    assert_eq!(scan.wallet_id(), wallet(1));
    let master = scan.master_key().unwrap();
    assert_eq!(master.depth, 0);
    // It is that wallet's master: it derives what the wallet's signer does.
    let p = path("m/9'/1'/5'/0'/0'/0'/0'");
    let identity = platform_signer(&v, SignerScope::PlatformIdentity);
    assert_eq!(
        master.derive_priv(&p).unwrap().private_key.public_key(),
        identity.public_key(&p).await.unwrap()
    );

    // Another wallet's grant, or another purpose, releases nothing.
    assert_eq!(
        v.scan_key(&wallet(2), &scan_token(&v)).unwrap_err(),
        VaultError::GrantPurposeMismatch
    );
    for purpose in [
        GrantPurpose::Spend { max_duffs: 1 },
        GrantPurpose::SignMessage,
        PLATFORM_OP,
    ] {
        let grant = v
            .authorize(purpose, Some(&wallet(1)), Credential::None)
            .unwrap();
        let token = v
            .redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
            .unwrap();
        assert_eq!(
            v.scan_key(&wallet(1), &token).unwrap_err(),
            VaultError::GrantPurposeMismatch
        );
    }

    // Lock: the scan key stops, a token redeemed before the lock releases
    // nothing, and no grant comes without the passphrase.
    let token = scan_token(&v);
    v.lock();
    assert_eq!(scan.master_key().unwrap_err(), SignerError::Locked);
    assert_eq!(
        v.scan_key(&wallet(1), &token).unwrap_err(),
        VaultError::Locked
    );
    assert_eq!(
        v.authorize(
            GrantPurpose::IdentityScan,
            Some(&wallet(1)),
            Credential::None
        )
        .unwrap_err(),
        VaultError::Locked
    );
    // With the passphrase, the grant's own key, held, serves the scan
    // until the next lock.
    let grant = v
        .authorize(
            GrantPurpose::IdentityScan,
            Some(&wallet(1)),
            Credential::Passphrase(PASS),
        )
        .unwrap();
    let mut tokens = [v
        .redeem_grant(&grant.id, GrantKind::IdentityScan, Some(&wallet(1)))
        .unwrap()];
    let hold = v.hold_key(&mut tokens).unwrap().unwrap();
    let scan = v.scan_key_held(&wallet(1), &hold, &tokens[0]).unwrap();
    assert_eq!(scan.master_key().unwrap().depth, 0);
    v.lock();
    assert_eq!(scan.master_key().unwrap_err(), SignerError::Locked);

    // Mixing-only unlock: no grant without the passphrase.
    v.unlock(PASS, UnlockScope::MixingOnly).unwrap();
    assert_eq!(
        v.authorize(
            GrantPurpose::IdentityScan,
            Some(&wallet(1)),
            Credential::None
        )
        .unwrap_err(),
        VaultError::MixingOnly
    );

    // Unencrypted: the grant needs no prompt.
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    let scan = v.scan_key(&wallet(1), &scan_token(&v)).unwrap();
    assert_eq!(scan.master_key().unwrap().depth, 0);
}

#[tokio::test]
async fn a_passphrase_platform_grant_on_a_locked_vault_lives_until_the_next_lock() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.lock();
    let grant = v
        .authorize(PLATFORM_OP, Some(&wallet(1)), Credential::Passphrase(PASS))
        .unwrap();
    let mut tokens = [v
        .redeem_grant(&grant.id, GrantKind::PlatformOp, Some(&wallet(1)))
        .unwrap()];
    let hold = v.hold_key(&mut tokens).unwrap().unwrap();
    let identity = v
        .platform_signer_held(&wallet(1), &hold, &tokens[0], SignerScope::PlatformIdentity)
        .unwrap();
    let key = path("m/9'/1'/5'/0'/0'/0'/0'");
    identity.public_key(&key).await.unwrap();
    // Still locked: no background signer, and lock() ends the grant's own key.
    assert_eq!(
        v.dashpay_crypto_signer(&wallet(1)).unwrap_err(),
        VaultError::Locked
    );
    v.lock();
    assert_eq!(
        identity.public_key(&key).await.unwrap_err(),
        SignerError::Locked
    );
}

#[tokio::test]
async fn identity_signatures_need_the_named_key_and_verify() {
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    let s = platform_signer(&v, SignerScope::PlatformIdentity);
    let data = b"state transition signable bytes";
    let digest: [u8; 32] = double_sha(data).try_into().unwrap();
    for key in 0..=5 {
        let p = path(&format!("m/9'/1'/5'/0'/0'/0'/{key}'"));
        let public = s.public_key(&p).await.unwrap();
        let hash = hash160::Hash::hash(&public.serialize());
        for key_data in [public.serialize().to_vec(), hash.as_byte_array().to_vec()] {
            assert!(s.identity_key_matches(&p, &key_data).unwrap());
            let sig = s.sign_identity(&p, &key_data, data).unwrap();
            // Compact recoverable, compressed flag (dashcore::signer::sign).
            let recid = RecoveryId::try_from(i32::from((sig[0] - 27) & 3)).unwrap();
            let rec = RecoverableSignature::from_compact(&sig[1..], recid).unwrap();
            assert!(sig[0] >= 31, "compressed flag");
            let msg = Message::from_digest(digest);
            assert_eq!(rec.recover_ecdsa(msg).unwrap(), public);
            public.verify(msg, &rec.to_standard()).unwrap();
        }
        // Another key's data: refused before signing.
        let other = s
            .public_key(&path(&format!("m/9'/1'/5'/0'/0'/1'/{key}'")))
            .await
            .unwrap();
        assert!(!s.identity_key_matches(&p, &other.serialize()).unwrap());
        assert!(matches!(
            s.sign_identity(&p, &other.serialize(), data),
            Err(SignerError::KeyMismatch(_))
        ));
        assert!(matches!(
            s.sign_identity(&p, &[], data),
            Err(SignerError::KeyMismatch(_))
        ));
    }
}

#[tokio::test]
async fn contact_info_opens_what_it_seals_and_refuses_foreign_ciphertext() {
    let fx = Fixture::new();
    let v = unencrypted_vault(&fx);
    let s = platform_signer(&v, SignerScope::DashPayCrypto);
    let root = path("m/9'/1'/5'/0'/0'/0'/2'");
    let sealed = s
        .contact_info_seal(&root, 3, &[0x33; 32], b"alias", &[0x11; 16])
        .unwrap();
    let opened = s
        .contact_info_open(&root, 3, &sealed.enc_to_user_id, &sealed.private_data)
        .unwrap();
    assert_eq!(opened.contact_id, [0x33; 32]);
    assert_eq!(&opened.private_data[..], b"alias");
    assert!(
        !format!("{opened:?}").contains("alias"),
        "Debug shows no plaintext"
    );
    // Another derivation index has other keys: the private data fails its
    // padding check (or, rarely, opens to garbage), and the id is not ours.
    assert_ne!(
        s.contact_info_open(&root, 4, &sealed.enc_to_user_id, &sealed.private_data)
            .map(|o| o.contact_id)
            .unwrap_or_default(),
        [0x33; 32]
    );
    // A derivation index that cannot be hardened.
    assert!(matches!(
        s.contact_info_seal(&root, 1 << 31, &[0; 32], b"", &[0; 16]),
        Err(SignerError::Derivation(_))
    ));
}
