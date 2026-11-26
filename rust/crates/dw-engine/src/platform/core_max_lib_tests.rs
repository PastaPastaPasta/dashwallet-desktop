//! Core Max's funding through the real library: the engine's read of BIP44
//! account 0 (`core_max::read`) and its drain signer under platform-wallet's
//! `DrainAll` build of that account (`PwShielded::fund_core_max`), on a
//! wallet whose coins the test credits. As in `shield_drain_lib_tests.rs`,
//! no SPV client runs, so a build that is signed stops at the broadcast;
//! the build itself is the one the drain signer approved.

use super::super::lease::FlowOutcome;
use super::super::lease::session_tests::{Fixture as Wallet, fixture as wallet_fixture};
use super::*;
use crate::WalletId;
use crate::platform::core_max::{self, CoreCoins, MaxPlan, MaxRefusal};
use crate::platform::flows::BudgetPurpose;
use crate::platform::platform_fund;
use crate::platform::registration_flow::PwLib;
use crate::platform::signers::DrainRefusal;
use dashcore::Transaction;
use key_wallet::wallet::managed_wallet_info::asset_lock_builder::AssetLockFundingAccount;
use platform_wallet::AssetLockFundingType;

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
    let arriving: Vec<_> = r.arriving.iter().map(|c| c.outpoint).collect();
    assert_eq!(arriving, [pending]);
    assert!(r.coins.iter().all(|c| c.outpoint != pending));
    let (result, signatures, _, _) = fund(&w, &r.coins);
    assert!(matches!(result, Err(FundError::NotSent(_))), "{result:?}");
    assert_eq!(signatures, 2);
}

/// Where a Core Max locks the account's coins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Dash wallet → Shielded: a `Shield` lease, the shield's key.
    Shield,
    /// Dash wallet → Platform ("Add from Dash wallet"): a
    /// `PlatformAddressFund` lease, the address top-up key.
    Fund,
}

impl Route {
    fn kind(self) -> FlowKind {
        match self {
            Self::Shield => FlowKind::Shield,
            Self::Fund => FlowKind::PlatformAddressFund,
        }
    }

    fn key(self) -> FundingKey {
        match self {
            Self::Shield => pool::SHIELD_KEY,
            Self::Fund => FundingKey {
                funding_type: AssetLockFundingType::AssetLockAddressTopUp,
                identity_index: 0,
                account: 0,
            },
        }
    }

    fn step(self, s: &NetworkSession, wallet: WalletId) -> String {
        match self {
            Self::Shield => pool::next_step(s, wallet).unwrap(),
            Self::Fund => platform_fund::next_step(s, wallet).unwrap(),
        }
    }
}

impl Route {
    /// The lock's floor, as the route's own Max has it.
    fn floor(self) -> u64 {
        match self {
            Self::Shield => drain::drain_floor(pool::shield_pool_fee(version(LATEST)).unwrap()),
            Self::Fund => platform_fund::minimum_duffs(version(LATEST)),
        }
    }
}

/// The engine's Core Max of the wallet as it stands on `route`: its
/// CoinJoin claim, the read of BIP44 account 0 and the plan of that read.
fn max_plan(w: &Wallet, route: Route) -> Result<MaxPlan, MaxRefusal> {
    let claim = core_max::claim(&w.session, w.wallet)?;
    let plan = core_max::plan(read(w), route.floor());
    drop(claim);
    plan
}

/// What one Core Max build signed: the call's result, the signatures
/// released, its outcome, and the transaction the builder showed the drain
/// signer.
struct Built {
    result: Result<OutPoint, FundError>,
    signatures: u64,
    outcome: FlowOutcome,
    tx: Option<Transaction>,
}

/// `plan` funded as `fund_core_max` funds it on `route` (`core_max::fund`):
/// the library's `DrainAll` build of BIP44 account 0 to the route's
/// credit script, under the call's drain signer, on a lease of a fresh
/// `PlatformOp` grant of exactly the plan's total, as the quote asks.
fn build(w: &Wallet, route: Route, plan: &MaxPlan) -> Built {
    let total = plan.total_duffs;
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
    let (s, wallet, coins) = (Arc::clone(&w.session), w.wallet, plan.coins.clone());
    w.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, route.kind(), &[grant]).await).unwrap();
        assert!(s.leases.refresh_funding(wallet).await);
        let step = route.step(&s, wallet);
        let fence = s.coin_fence(wallet).await.unwrap();
        let key = route.key();
        let permit = s
            .drain_call_permit(&lease, step.clone(), total, step, key, fence)
            .await
            .unwrap();
        let (_, pw) = PwLib::open(&s, wallet).await.unwrap();
        let script = core_max::credit_script(&pw, key.funding_type)
            .await
            .unwrap();
        let signer =
            drain::drain_signer(&permit, &coins, total, route.floor(), script, true).unwrap();
        let account = AssetLockFundingAccount::Bip44 { account_index: 0 };
        let result = pw
            .fund_drain_with(account, key, route.floor(), &signer)
            .await;
        let tx = signer.drain_shown();
        let signatures = permit.signatures().released();
        let outcome = permit.finish(result.is_ok());
        Built {
            result,
            signatures,
            outcome,
            tx,
        }
    })
}

/// BIP44 account 0's coins, each with its value.
fn account_0(w: &Wallet) -> Vec<(OutPoint, u64)> {
    let (s, wallet) = (Arc::clone(&w.session), w.wallet);
    w.engine.block_on(async move {
        let pw = s.wallet(&wallet).await.unwrap();
        let state = crate::wallet_read::info_of(&pw).await.unwrap();
        (state.core_wallet.accounts.standard_bip44_accounts.get(&0))
            .map(|a| a.utxos.values().map(|u| (u.outpoint, u.value())).collect())
            .unwrap_or_default()
    })
}

/// What stays spendable in BIP44 account 0 once `tx` confirms: every coin
/// of it `tx` does not spend, plus whatever `tx` pays back to the wallet
/// (change).
fn remainder(w: &Wallet, tx: &Transaction) -> u64 {
    let spent: std::collections::HashSet<OutPoint> =
        tx.input.iter().map(|i| i.previous_output).collect();
    let kept: u64 = (account_0(w).into_iter())
        .filter(|(o, _)| !spent.contains(o))
        .map(|(_, v)| v)
        .sum();
    let change: u64 = (tx.output.iter())
        .filter(|o| !o.script_pubkey.is_op_return())
        .map(|o| o.value)
        .sum();
    kept + change
}

/// The lock's credit: its asset-lock payload's one credit output.
fn credit_of(tx: &Transaction) -> u64 {
    use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
    let Some(TransactionPayload::AssetLockPayloadType(payload)) = &tx.special_transaction_payload
    else {
        panic!("not an asset lock: {tx:?}");
    };
    payload.credit_outputs.iter().map(|o| o.value).sum()
}

/// The signed lock of a Core Max: every planned coin in, exactly one output
/// (the burn of the credit, no change), the plan's total exactly what it
/// spends and the fee out of it, within the quoted bound; BIP44 account 0
/// left at `left`.
fn assert_drained(w: &Wallet, plan: &MaxPlan, built: &Built, left: u64) {
    let Err(FundError::NotSent(e)) = &built.result else {
        panic!("{:?}", built.result);
    };
    assert!(e.to_string().contains("client not started"), "{e}");
    assert_eq!(built.outcome, FlowOutcome::MaybeSent);
    assert_eq!(built.signatures, plan.coins.len() as u64);
    let tx = built
        .tx
        .as_ref()
        .expect("the drain signer approved a build");
    let mut inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    let mut planned: Vec<OutPoint> = plan.coins.iter().map(|c| c.outpoint).collect();
    inputs.sort_unstable();
    planned.sort_unstable();
    assert_eq!(inputs, planned, "the lock spends exactly the planned coins");
    assert_eq!(tx.output.len(), 1, "no change output: {:?}", tx.output);
    assert!(tx.output[0].script_pubkey.is_op_return());
    let credit = credit_of(tx);
    assert_eq!(tx.output[0].value, credit);
    let fee = plan.total_duffs - credit;
    assert!(
        fee > 0 && fee <= plan.fee_duffs,
        "fee {fee}, bound {}",
        plan.fee_duffs
    );
    assert!(credit >= plan.duffs, "the credit is at least the quote's");
    assert_eq!(remainder(w, tx), left);
}

/// Two BIP44 coins, where Max by amount left 633 duffs of change in the
/// Dash wallet (`a_build_with_a_change_output_is_refused_unsigned`): Max
/// to Shielded and Max to Platform each lock both coins with the fee out
/// of the lock and no change, so account 0 is left at exactly 0.
#[test]
fn a_library_core_max_leaves_account_0_at_exactly_zero() {
    for route in [Route::Shield, Route::Fund] {
        let w = wallet_fixture(false);
        w.credit(0x81, COIN);
        w.credit(0x82, COIN / 2);
        let plan = max_plan(&w, route).unwrap();
        assert_eq!(plan.total_duffs, COIN + COIN / 2, "{route:?}");
        assert_eq!(plan.max.inputs, 2);
        assert_eq!((plan.max.left_duffs, plan.max.arriving_duffs), (0, 0));
        let built = build(&w, route, &plan);
        assert_drained(&w, &plan, &built, 0);
    }
}

/// Max by amount, as the sheet sent it before: the most the amount path's
/// quote leaves room for (everything less its worst-case fee bound). On
/// two BIP44 coins that build carries a change output of 633 duffs, the
/// dust a Max to Shielded left in the Dash wallet; the drain signer
/// refuses it before any signature: nothing signed, nothing tracked.
#[test]
fn a_build_with_a_change_output_is_refused_unsigned() {
    for route in [Route::Shield, Route::Fund] {
        let w = wallet_fixture(false);
        w.credit(0x81, COIN);
        w.credit(0x82, COIN / 2);
        let plan = max_plan(&w, route).unwrap();
        let (s, wallet, coins) = (Arc::clone(&w.session), w.wallet, plan.coins.clone());
        let total = plan.total_duffs;
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
        let (result, refusal, shown, signatures, outcome, tracked) =
            w.engine.block_on(async move {
                let lease = (s.begin_lease(wallet, route.kind(), &[grant]).await).unwrap();
                assert!(s.leases.refresh_funding(wallet).await);
                let step = route.step(&s, wallet);
                let fence = s.coin_fence(wallet).await.unwrap();
                let key = route.key();
                let permit = s
                    .drain_call_permit(&lease, step.clone(), total, step, key, fence)
                    .await
                    .unwrap();
                let (_, pw) = PwLib::open(&s, wallet).await.unwrap();
                let script = core_max::credit_script(&pw, key.funding_type)
                    .await
                    .unwrap();
                let signer =
                    drain::drain_signer(&permit, &coins, total, route.floor(), script, true)
                        .unwrap();
                let amount = total - s.fee_bound_worst(wallet).await.unwrap();
                let result =
                    (pw.fund_lock_with(key.funding_type, amount, key.identity_index, &signer))
                        .await;
                let refusal = signer.drain_refusal();
                let shown = signer.drain_shown();
                let signatures = permit.signatures().released();
                let outcome = permit.finish(result.is_ok());
                let tracked = pw.wallet.asset_locks().list_tracked_locks().await.len();
                (result, refusal, shown, signatures, outcome, tracked)
            });
        let tx = shown.expect("the builder showed the signer its build");
        let change: Vec<u64> = (tx.output.iter())
            .filter(|o| !o.script_pubkey.is_op_return())
            .map(|o| o.value)
            .collect();
        assert_eq!(change, [633], "{route:?}: {:?}", tx.output);
        assert_eq!(tx.input.len(), 2, "{route:?}");
        assert!(result.is_err(), "{route:?}: {result:?}");
        assert_eq!(refusal, Some(DrainRefusal::CoinsChanged), "{route:?}");
        assert_eq!(signatures, 0, "{route:?}");
        assert_eq!(outcome, FlowOutcome::Cancelled, "{route:?}");
        assert_eq!(tracked, 0, "{route:?}");
    }
}

/// Coins in another account (CoinJoin) stay where they are and are
/// reported as left; account 0 is drained to 0.
#[test]
fn left_coins_stay_and_are_reported() {
    let w = wallet_with_coins();
    w.credit_coinjoin(0x86, COIN / 4);
    let plan = max_plan(&w, Route::Shield).unwrap();
    assert_eq!(plan.max.left_duffs, COIN / 10 + 100 + COIN / 4);
    let built = build(&w, Route::Shield, &plan);
    let tx = built.tx.as_ref().unwrap();
    let spent: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    assert_eq!(spent.len(), 2, "only account 0's coins");
    assert_drained(&w, &plan, &built, 0);
}

/// A coin not final yet is reported arriving and is all account 0 keeps.
#[test]
fn an_arriving_coin_is_reported_and_is_all_that_stays() {
    let w = wallet_with_coins();
    let pending = w.credit(0x85, COIN / 5);
    let (s, wallet) = (Arc::clone(&w.session), w.wallet);
    w.engine.block_on(async move {
        let pw = s.wallet(&wallet).await.unwrap();
        let mut state = pw.state_mut().await;
        let account = (state.core_wallet.accounts.standard_bip44_accounts)
            .get_mut(&0)
            .unwrap();
        account.utxos.get_mut(&pending).unwrap().is_confirmed = false;
    });
    let plan = max_plan(&w, Route::Fund).unwrap();
    assert_eq!(plan.max.arriving_duffs, COIN / 5);
    let built = build(&w, Route::Fund, &plan);
    assert_drained(&w, &plan, &built, COIN / 5);
}

/// A held coin of account 0 (here reserved by another spend) refuses Max
/// before any build.
#[test]
fn a_held_coin_refuses_max_before_any_build() {
    let w = wallet_with_coins();
    let held = account_0(&w)[0].0;
    let owner = crate::send::SpendOwner::Tx(dashcore::Txid::from_byte_array([0x99; 32]));
    w.session.spends.reserve(w.wallet, owner, [held]);
    assert_eq!(max_plan(&w, Route::Shield), Err(MaxRefusal::CoinsLocked));
    w.session.spends.release(&w.wallet, owner);
    assert!(max_plan(&w, Route::Shield).is_ok(), "released, Max runs");
}

/// 51 coins in account 0 refuse Max before any build; at 50 the lock takes
/// all 50 and leaves account 0 at 0.
#[test]
fn past_50_coins_refuses_max_and_50_drain_to_zero() {
    let w = wallet_fixture(false);
    for i in 0..50u8 {
        w.credit(0x10 + i, COIN / 100);
    }
    let plan = max_plan(&w, Route::Fund).unwrap();
    assert_eq!(plan.max.inputs, 50);
    let built = build(&w, Route::Fund, &plan);
    assert_drained(&w, &plan, &built, 0);

    let w = wallet_fixture(false);
    for i in 0..51u8 {
        w.credit(0x10 + i, COIN / 100);
    }
    assert_eq!(
        max_plan(&w, Route::Shield),
        Err(MaxRefusal::TooManyInputs {
            inputs: 51,
            max: 50
        })
    );
}

/// While the wallet mixes, Max is refused before it reads the account;
/// once mixing stops it runs.
#[test]
fn mixing_refuses_max() {
    let w = wallet_with_coins();
    w.session.coinjoin.set_mixing_for_test(w.wallet, true);
    assert_eq!(max_plan(&w, Route::Shield), Err(MaxRefusal::MixingActive));
    w.session.coinjoin.set_mixing_for_test(w.wallet, false);
    assert!(max_plan(&w, Route::Shield).is_ok());
}
