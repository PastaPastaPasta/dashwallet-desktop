//! The Orchard keys of the shielded scopes (0.1 E1, `src/shielded.rs`): the
//! derivation iOS and Android use, the grant the spend authority needs,
//! and the lock rules both follow.

mod common;

use common::*;
use dpp::address_funds::OrchardAddress;
use dw_vault::{Credential, GrantPurpose, GrantToken, SignerError, SignerScope, Vault, VaultError};
use key_wallet::Network;
use platform_wallet::wallet::shielded::AccountViewingKeys;

const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon \
                       abandon abandon about";
/// Lane V's reference (dw-rel `v/REPORT.md`): iOS and Android 12.0.0 show
/// this shielded address for "abandon … about" on mainnet.
const ABANDON_MAINNET: &str =
    "dash1zrlpvqcgyx2rag5d5rkhq0wy69hz3q37xe54xlrreqgp8htpv3gmm2l52gznxha0fd3kq9s4l67rj";

/// An unencrypted vault on `network` holding the "abandon … about" wallet
/// (no BIP39 passphrase) as wallet 1.
fn abandon_vault(fx: &Fixture, network: Network, name: &str) -> Vault {
    let v = Vault::open(fx.vault_dir(), network, name, fx.config()).unwrap();
    v.create(None).unwrap();
    let secret = dw_vault::mnemonic::derive_secret(ABANDON.as_bytes(), b"", false).unwrap();
    v.store_wallet_secret(&wallet(1), &secret).unwrap();
    v
}

fn token(v: &Vault, purpose: GrantPurpose) -> GrantToken {
    let grant = v
        .authorize(purpose, Some(&wallet(1)), Credential::None)
        .unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
        .unwrap()
}

fn address(fvk: &[u8; 96], network: Network) -> String {
    let views = AccountViewingKeys::from_fvk_bytes(fvk).unwrap();
    let raw = views.default_address.to_raw_address_bytes();
    OrchardAddress::from_raw_bytes(&raw)
        .unwrap()
        .to_bech32m_string(network)
}

#[test]
fn the_abandon_wallet_has_the_mobile_wallets_mainnet_address() {
    let fx = Fixture::new();
    let v = abandon_vault(&fx, Network::Mainnet, "mainnet");
    let fvk = v.shielded_viewing_key(&wallet(1)).unwrap();
    assert_eq!(address(&fvk, Network::Mainnet), ABANDON_MAINNET);

    // The spend key derives the same account.
    let spend = v
        .shielded_spend_key(&wallet(1), &token(&v, platform_op(0, 1)))
        .unwrap();
    assert_eq!(spend.wallet_id(), wallet(1));
    assert_eq!(spend.keys().unwrap().viewing_keys().to_fvk_bytes(), fvk);
}

#[test]
fn testnet_derives_under_coin_type_1() {
    let fx = Fixture::new();
    let main = abandon_vault(&fx, Network::Mainnet, "mainnet");
    let fx2 = Fixture::new();
    let test = abandon_vault(&fx2, Network::Testnet, "testnet");
    let (m, t) = (
        main.shielded_viewing_key(&wallet(1)).unwrap(),
        test.shielded_viewing_key(&wallet(1)).unwrap(),
    );
    assert_ne!(m, t);
    assert!(address(&t, Network::Testnet).starts_with("tdash1z"));
}

fn platform_op(max_duffs: u64, max_credits: u64) -> GrantPurpose {
    GrantPurpose::PlatformOp {
        max_duffs,
        max_credits,
    }
}

#[test]
fn the_spend_key_needs_a_platform_op_grant_with_credits() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_wallet_secret(&wallet(2), &secret(2)).unwrap();
    let mismatch = VaultError::GrantPurposeMismatch;

    let ok = token(&v, platform_op(0, 5));
    let key = v.shielded_spend_key(&wallet(1), &ok).unwrap();
    key.keys().unwrap();
    // Another wallet's token, a token without credits, another purpose.
    assert_eq!(v.shielded_spend_key(&wallet(2), &ok).unwrap_err(), mismatch);
    let no_credits = token(&v, platform_op(1_000, 0));
    assert_eq!(
        v.shielded_spend_key(&wallet(1), &no_credits).unwrap_err(),
        mismatch
    );
    let spend = token(&v, GrantPurpose::Spend { max_duffs: 1 });
    assert_eq!(
        v.shielded_spend_key(&wallet(1), &spend).unwrap_err(),
        mismatch
    );
    // The Platform signers do not hand out the shielded scopes.
    for scope in [SignerScope::ShieldedSpend, SignerScope::ShieldedView] {
        assert!(matches!(
            v.platform_signer(&wallet(1), &ok, scope).unwrap_err(),
            VaultError::InvalidArgument(_)
        ));
    }

    // A lock stops the key, and a token redeemed before it releases none.
    let before = token(&v, platform_op(0, 5));
    v.lock();
    assert!(matches!(key.keys(), Err(SignerError::Locked)));
    assert_eq!(
        v.shielded_spend_key(&wallet(1), &before).unwrap_err(),
        VaultError::Locked
    );
}

#[test]
fn the_viewing_key_needs_the_full_key_without_a_prompt() {
    let fx = Fixture::new();
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    // Unlocked with scope Full (the create's unlock): no prompt needed.
    let fvk = v.shielded_viewing_key(&wallet(1)).unwrap();
    v.lock();
    assert_eq!(
        v.shielded_viewing_key(&wallet(1)).unwrap_err(),
        VaultError::Locked
    );
    // A wallet without a seed has none.
    assert_eq!(
        abandon_vault(&Fixture::new(), Network::Regtest, "regtest")
            .shielded_viewing_key(&wallet(3))
            .unwrap_err(),
        VaultError::NoSecret
    );
    assert_ne!(fvk, [0; 96]);
}
