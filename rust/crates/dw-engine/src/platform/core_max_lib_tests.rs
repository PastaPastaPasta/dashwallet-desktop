//! Core Max's funding through the real library: the engine's read of BIP44
//! account 0 (`core_max::read`) and its drain signer under platform-wallet's
//! `DrainAll` build of that account (`PwShielded::fund_core_max`), on a
//! wallet whose coins the test credits. As in `shield_drain_lib_tests.rs`,
//! no SPV client runs, so a build that is signed stops at the broadcast.

use super::super::lease::FlowOutcome;
use super::super::lease::session_tests::{Fixture as Wallet, fixture as wallet_fixture};
use super::*;
use crate::platform::core_max::{self, CoreCoins};
use crate::platform::flows::BudgetPurpose;

const COIN: u64 = 100_000_000;

/// Two confirmed BIP44 coins and a CoinJoin coin.
fn wallet_with_coins() -> Wallet {
    let w = wallet_fixture(false);
    w.credit(0x81, COIN);
    w.credit(0x82, COIN / 2);
    w.credit_coinjoin(0x83, COIN / 10 + 100);
    w
}

fn read(w: &Wallet) -> CoreCoins {
    w.engine.block_on(async {
        let fence = w.session.coin_fence(w.wallet).await.unwrap();
        core_max::read(&w.session, &fence).await.unwrap()
    })
}

/// One Core Max funding of `coins` under a `Shield` lease of a fresh
/// `PlatformOp` grant of their sum: its result, the signatures released,
/// the call's outcome and the lease's Funding budget before and after.
fn fund(
    w: &Wallet,
    coins: &[DrainCoin],
) -> (Result<OutPoint, FundError>, u64, FlowOutcome, (u64, u64)) {
    let total: u64 = coins.iter().map(|c| c.value).sum();
    let grant = (w.session.vault())
        .authorize(
            dw_vault::GrantPurpose::PlatformOp {
                max_duffs: total,
                max_credits: 0,
            },
            Some(&w.wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let (s, wallet, coins) = (Arc::clone(&w.session), w.wallet, coins.to_vec());
    let floor = drain::drain_floor(pool::shield_pool_fee(version(LATEST)).unwrap());
    w.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant]).await).unwrap();
        let before = lease.available(BudgetPurpose::Funding).unwrap();
        assert!(s.leases.refresh_funding(wallet).await);
        let step = pool::next_step(&s, wallet).unwrap();
        let fence = s.coin_fence(wallet).await.unwrap();
        let permit = s
            .drain_call_permit(&lease, step.clone(), total, step, pool::SHIELD_KEY, fence)
            .await
            .unwrap();
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let result = lib.fund_core_max(&coins, total, floor, &permit).await;
        let signatures = permit.signatures().released();
        let outcome = permit.finish(result.is_ok());
        let after = lease.available(BudgetPurpose::Funding).unwrap();
        (result, signatures, outcome, (before, after))
    })
}

/// The read takes BIP44 account 0's coins and leaves the CoinJoin coin;
/// the library's drain of them is admitted by the drain signer, which
/// signs each planned coin once, and stops at the broadcast (no SPV).
#[test]
fn the_library_core_max_signs_exactly_the_planned_coins() {
    let w = wallet_with_coins();
    let r = read(&w);
    let mut got: Vec<u64> = r.coins.iter().map(|c| c.value).collect();
    got.sort_unstable();
    assert_eq!(got, [COIN / 2, COIN]);
    assert!(r.coins.iter().all(|c| !c.held), "{r:?}");
    assert_eq!(r.left_duffs, COIN / 10 + 100);
    let (result, signatures, outcome, _) = fund(&w, &r.coins);
    let Err(FundError::NotSent(e)) = &result else {
        panic!("{result:?}");
    };
    assert!(e.to_string().contains("client not started"), "{e}");
    assert_eq!(signatures, 2, "one signature per planned coin");
    assert_eq!(outcome, FlowOutcome::MaybeSent);
}

/// A coin that came after the plan: the library's drain takes it too, so
/// the build is not the plan and is refused before any signature, its
/// Funding charge refunded.
#[test]
fn a_coin_after_the_plan_stops_the_library_core_max_unsigned() {
    let w = wallet_with_coins();
    let r = read(&w);
    w.credit(0x84, COIN);
    let (result, signatures, outcome, (before, after)) = fund(&w, &r.coins);
    assert_eq!(result, Err(FundError::CoinsChanged));
    assert_eq!(signatures, 0);
    assert_eq!(outcome, FlowOutcome::Cancelled);
    assert_eq!(before, after, "the charge refunded");
}

/// A coin not final yet is left out of the plan and of the library's
/// build alike, and counted arriving.
#[test]
fn an_unconfirmed_coin_is_arriving_and_never_signed() {
    let w = wallet_with_coins();
    let pending = w.credit(0x85, COIN);
    let (s, wallet) = (Arc::clone(&w.session), w.wallet);
    w.engine.block_on(async move {
        let pw = s.wallet(&wallet).await.unwrap();
        let mut state = pw.state_mut().await;
        let account = (state.core_wallet.accounts.standard_bip44_accounts)
            .get_mut(&0)
            .unwrap();
        account.utxos.get_mut(&pending).unwrap().is_confirmed = false;
    });
    let r = read(&w);
    assert_eq!(r.arriving_duffs, COIN);
    assert!(r.coins.iter().all(|c| c.outpoint != pending));
    let (result, signatures, _, _) = fund(&w, &r.coins);
    assert!(matches!(result, Err(FundError::NotSent(_))), "{result:?}");
    assert_eq!(signatures, 2);
}
