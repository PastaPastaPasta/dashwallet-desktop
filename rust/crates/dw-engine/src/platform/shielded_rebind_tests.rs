//! C8b: the shielded viewing key over the real `PwShielded::bind`, with the
//! vault locked. A key persisted by an earlier bind rebinds at the Platform
//! start after a reopen and a pass is admitted, the vault locked throughout; a
//! wallet never bound reads `shielded.not_synced{account}` while the vault
//! is locked and binds once it is unlocked.

use std::sync::Arc;

use dashcore::Network;
use dpp::address_funds::OrchardAddress;
use dw_vault::{LockState, UnlockScope, VaultError};
use platform_wallet::wallet::shielded::AccountViewingKeys;

use super::ShieldedError;
use super::shielded_pool::{self as pool, ShieldedLib, on_pool};
use super::topup_tests::{PASS, fixture, options};
use crate::{DashNetwork, Engine, NetworkSession, WalletId};

/// The account's address from the vault's viewing key (vault unlocked).
fn expected_address(session: &NetworkSession, wallet: WalletId) -> String {
    let fvk = session.vault.shielded_viewing_key(&wallet.0).unwrap();
    let views = AccountViewingKeys::from_fvk_bytes(&fvk).unwrap();
    OrchardAddress::from_raw_bytes(&views.default_address.to_raw_address_bytes())
        .unwrap()
        .to_bech32m_string(Network::Regtest)
}

fn lock(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    engine.block_on(session.lock_vault()).unwrap();
    assert_ne!(session.vault.lock_state(), LockState::Unlocked);
    assert_eq!(
        session.vault.shielded_viewing_key(&wallet.0),
        Err(VaultError::Locked)
    );
}

/// Whether the coordinator's passes cover `wallet`'s shielded account.
fn registered(engine: &Engine, session: &NetworkSession, wallet: WalletId) -> bool {
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let Some(coordinator) = manager.shielded_coordinator().await else {
            return false;
        };
        (coordinator.registered_subwallets().await)
            .iter()
            .any(|s| s.wallet_id == wallet.0 && s.account_index == dw_vault::SHIELDED_ACCOUNT)
    })
}

/// A refusal as the host reads it: `not_synced:<reason>`, or the code.
fn wait_of<T: std::fmt::Debug>(r: Result<T, ShieldedError>) -> String {
    match r {
        Err(ShieldedError::NotSynced { reason }) => format!("not_synced:{reason}"),
        Ok(v) => format!("ok:{v:?}"),
        Err(e) => e.code().to_owned(),
    }
}

#[test]
fn a_persisted_viewing_key_rebinds_at_the_start_and_a_pass_is_admitted_with_the_vault_locked() {
    let mut f = fixture(true);
    let want = expected_address(&f.session, f.wallet);
    let first = f
        .engine
        .block_on(f.session.dashpay(f.wallet).shielded_receive_address());
    assert_eq!(first.unwrap().address, want);

    f.engine
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.session = (f.engine)
        .block_on(f.engine.open_network(DashNetwork::Regtest, options()))
        .unwrap();
    lock(&f.engine, &f.session, f.wallet);
    let manager = f.session.manager().unwrap();
    assert!(
        f.engine.block_on(manager.shielded_coordinator()).is_none(),
        "no shielded store is open before the start"
    );

    let admitted: Vec<WalletId> = [f.wallet]
        .into_iter()
        .filter(|w| {
            f.session
                .platform
                .admit(w, || f.session.vault.has_wallet_secret(&w.0))
                == Some(true)
        })
        .collect();
    assert_eq!(admitted, [f.wallet], "admitted with the vault locked");
    let s = Arc::clone(&f.session);
    let admitted_pass = f.engine.block_on(async move {
        let (_cancel, cancelled) = tokio::sync::watch::channel(false);
        pool::start(&s, &manager, &admitted, &cancelled).await;
        pool::guarded_pass(&s, &manager, true).await
    });
    assert!(
        registered(&f.engine, &f.session, f.wallet),
        "the start rebound the account from wallet.sqlite"
    );
    assert!(admitted_pass, "a pass was admitted with the vault locked");

    let dp = f.session.dashpay(f.wallet);
    let address = f.engine.block_on(dp.shielded_receive_address()).unwrap();
    assert_eq!(address.address, want);
    assert_ne!(f.session.vault.lock_state(), LockState::Unlocked);
}

#[test]
fn a_first_bind_while_locked_is_not_synced_and_binds_after_the_unlock() {
    let f = fixture(true);
    let want = expected_address(&f.session, f.wallet);
    lock(&f.engine, &f.session, f.wallet);

    let bound = f
        .engine
        .block_on(on_pool(&f.session, f.wallet, |_, lib| async move {
            Ok((lib.bind().await?, lib.address().await))
        }));
    assert_eq!(bound.unwrap(), (false, None));
    let dp = f.session.dashpay(f.wallet);
    assert_eq!(
        wait_of(f.engine.block_on(dp.shielded_receive_address())),
        "not_synced:account"
    );
    let status = f.engine.block_on(dp.shielded_status()).unwrap();
    assert!(status.active && !status.ready, "{status:?}");
    assert!(!registered(&f.engine, &f.session, f.wallet));

    f.engine
        .block_on(f.session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    let address = f.engine.block_on(dp.shielded_receive_address()).unwrap();
    assert_eq!(address.address, want);
    assert!(registered(&f.engine, &f.session, f.wallet));
}
