//! The move to Shielded's funding through the real library: the engine's
//! drain signer under platform-wallet's `DrainAll` build of CoinJoin
//! account 0 (`PwShielded::fund_drain`, `PwLib::fund_drain_with`), on a
//! wallet whose coins the test credits.
//!
//! No SPV client runs here, so the library's broadcast is refused before
//! anything is sent ("SPV broadcast not sent: client not started",
//! platform-wallet `spv/runtime.rs` `broadcast_transaction_and_wait`).
//! Every test stops there at the latest: the lock is built, signed and
//! tracked, then untracked by the library on that refusal. The apply to
//! Shielded is never reached.

use super::super::lease::FlowOutcome;
use super::super::lease::session_tests::{Fixture as Wallet, fixture as wallet_fixture};
use super::super::registration_flow::PwLib;
use super::*;
use crate::platform::flows::BudgetPurpose;
use key_wallet::Utxo;

const COIN: u64 = 100_000_000;

/// What one funding call of a drain left.
#[derive(Debug)]
struct Funded {
    result: Result<OutPoint, FundError>,
    signatures: u64,
    outcome: FlowOutcome,
    /// The lease's Funding budget before the call and after its end.
    budget: (u64, u64),
    tracked: usize,
}

impl Funded {
    /// Refused before any signature: nothing signed, tracked or sent, the
    /// call `Cancelled` and its charge back (the grant's budget unused).
    fn assert_refused_unsigned(&self, error: FundError) {
        assert_eq!(self.result, Err(error), "{self:?}");
        assert_eq!(self.signatures, 0, "no signature released");
        assert_eq!(self.outcome, FlowOutcome::Cancelled);
        assert_eq!(self.budget.0, self.budget.1, "the grant's budget unused");
        assert_eq!(self.tracked, 0, "no lock tracked, so none sent");
    }
}

/// Three confirmed CoinJoin coins of the wallet.
fn wallet_with_coins() -> Wallet {
    let w = wallet_fixture(false);
    w.credit_coinjoin(0x61, COIN + 1_000);
    w.credit_coinjoin(0x62, COIN + 1_000);
    w.credit_coinjoin(0x63, COIN / 10 + 100);
    w
}

/// The plan the engine reads now: the account's coins under the fence.
fn plan(w: &Wallet) -> Vec<DrainCoin> {
    w.engine.block_on(async {
        let fence = w.session.coin_fence(w.wallet).await.unwrap();
        drain::read_drain_coins(&w.session, &fence).await.unwrap()
    })
}

/// The pool's minimum lock, as `drain` passes it.
fn floor() -> u64 {
    drain::drain_floor(pool::shield_pool_fee(version(LATEST)).unwrap())
}

/// One drain funding of `coins` (the plan) under a `ShieldDrain` lease of
/// a fresh `Spend` grant of their sum, `call` making the library call.
fn fund_with<F, Fut>(w: &Wallet, coins: &[DrainCoin], call: F) -> Funded
where
    F: FnOnce(Arc<NetworkSession>, Vec<DrainCoin>, u64, CallPermit) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = (Result<OutPoint, FundError>, CallPermit)> + Send,
{
    let total: u64 = coins.iter().map(|c| c.value).sum();
    let grant = (w.session.vault())
        .authorize(
            dw_vault::GrantPurpose::Spend { max_duffs: total },
            Some(&w.wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let (s, wallet, coins) = (Arc::clone(&w.session), w.wallet, coins.to_vec());
    w.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, FlowKind::ShieldDrain, &[grant]).await).unwrap();
        let before = lease.available(BudgetPurpose::Funding).unwrap();
        let step = pool::next_step(&s, wallet).unwrap();
        let fence = s.coin_fence(wallet).await.unwrap();
        let permit = s
            .drain_call_permit(&lease, step.clone(), total, step, pool::SHIELD_KEY, fence)
            .await
            .unwrap();
        let (result, permit) = call(Arc::clone(&s), coins, total, permit).await;
        let signatures = permit.signatures().released();
        let outcome = permit.finish(result.is_ok());
        let after = lease.available(BudgetPurpose::Funding).unwrap();
        let pw = s.wallet(&wallet).await.unwrap();
        let tracked = pw.asset_locks().list_tracked_locks().await.len();
        Funded {
            result,
            signatures,
            outcome,
            budget: (before, after),
            tracked,
        }
    })
}

/// [`fund_with`] through `PwShielded::fund_drain`, the drain's own call.
fn fund(w: &Wallet, coins: &[DrainCoin], minimum_lock_duffs: u64) -> Funded {
    let wallet = w.wallet;
    fund_with(w, coins, move |s, coins, total, permit| async move {
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let result = lib
            .fund_drain(&coins, total, minimum_lock_duffs, &permit)
            .await;
        (result, permit)
    })
}

/// Edits the wallet's CoinJoin account 0 coin at `outpoint` (or adds one
/// there, at `like`'s address).
fn set_coin(
    w: &Wallet,
    outpoint: OutPoint,
    like: Option<OutPoint>,
    edit: impl FnOnce(&mut Utxo) + Send + 'static,
) {
    let (s, wallet) = (Arc::clone(&w.session), w.wallet);
    w.engine.block_on(async move {
        let pw = s.wallet(&wallet).await.unwrap();
        let mut state = pw.state_mut().await;
        let account = (state.core_wallet.accounts.coinjoin_accounts)
            .get_mut(&0)
            .unwrap();
        let mut utxo = match like {
            Some(from) => {
                let mut u = account.utxos.get(&from).unwrap().clone();
                u.outpoint = outpoint;
                u
            }
            None => account.utxos.get(&outpoint).unwrap().clone(),
        };
        edit(&mut utxo);
        account.utxos.insert(outpoint, utxo);
    });
}

fn drop_coin(w: &Wallet, outpoint: OutPoint) {
    let (s, wallet) = (Arc::clone(&w.session), w.wallet);
    w.engine.block_on(async move {
        let pw = s.wallet(&wallet).await.unwrap();
        let mut state = pw.state_mut().await;
        (state.core_wallet.accounts.coinjoin_accounts)
            .get_mut(&0)
            .unwrap()
            .utxos
            .remove(&outpoint);
    });
}

/// The library's drain of the planned coins: its builder takes exactly
/// them, the drain signer admits the build and signs each coin once, and
/// the library then refuses its own broadcast (no SPV client): that
/// refusal comes after the signatures, so the call reads possibly sent
/// (`MaybeSent`, the charge kept) though the library untracked the lock.
#[test]
fn the_library_drain_of_the_plan_signs_every_coin_and_stops_at_the_broadcast() {
    let w = wallet_with_coins();
    let coins = plan(&w);
    assert_eq!(coins.len(), 3);
    let f = fund(&w, &coins, floor());
    let Err(FundError::NotSent(e)) = &f.result else {
        panic!("{f:?}");
    };
    assert!(
        e.to_string().contains("client not started"),
        "stopped at the broadcast: {e}"
    );
    assert_eq!(f.signatures, 3, "one signature per planned coin");
    assert_eq!(f.outcome, FlowOutcome::MaybeSent);
    assert!(f.budget.1 < f.budget.0, "the charge stays");
    assert_eq!(f.tracked, 0, "the library untracked the refused lock");
}

/// A planned coin replaced by a larger one at the same
/// address (a reorg or a replay) is a build the plan did not price. The
/// library's selection takes the new coin; the signer refuses the build
/// when the builder shows it, before any signature.
#[test]
fn a_larger_coin_at_a_planned_address_stops_the_library_drain_unsigned() {
    let w = wallet_with_coins();
    let coins = plan(&w);
    let old = coins[0].outpoint;
    let new = OutPoint::new(Txid::from_byte_array([0x71; 32]), 0);
    set_coin(&w, new, Some(old), |u| u.txout.value += COIN);
    drop_coin(&w, old);
    assert_eq!(
        plan(&w).iter().find(|c| c.outpoint == new).unwrap().path,
        coins[0].path
    );
    fund(&w, &coins, floor()).assert_refused_unsigned(FundError::CoinsChanged);
}

/// A planned coin that lost its block (now only InstantSend-locked) is
/// one the drain must not spend, whatever the library's selection takes.
#[test]
fn a_planned_coin_that_lost_its_confirmation_stops_the_library_drain_unsigned() {
    let w = wallet_with_coins();
    let coins = plan(&w);
    set_coin(&w, coins[2].outpoint, None, |u| {
        u.is_confirmed = false;
        u.is_instantlocked = true;
    });
    fund(&w, &coins, floor()).assert_refused_unsigned(FundError::CoinsChanged);
}

/// A coin that arrived after the plan: the library's drain takes every
/// coin, so the build is not the plan's.
#[test]
fn a_coin_after_the_plan_stops_the_library_drain_unsigned() {
    let w = wallet_with_coins();
    let coins = plan(&w);
    w.credit_coinjoin(0x64, COIN);
    fund(&w, &coins, floor()).assert_refused_unsigned(FundError::CoinsChanged);
}

/// A plan whose credit is below the library's drain minimum: the signer
/// refuses it before any signature, where the library itself would only
/// judge it after signing (next test).
#[test]
fn a_credit_below_the_drain_minimum_is_refused_before_any_signature() {
    let w = wallet_with_coins();
    let coins = plan(&w);
    let total: u64 = coins.iter().map(|c| c.value).sum();
    fund(&w, &coins, total).assert_refused_unsigned(FundError::BelowMinimum { min: total });
}

/// The library's own minimum, behind a signer that does not check it:
/// the build is signed first, then dropped below the minimum ("nothing
/// was broadcast", `build.rs` `DrainAll`), so the call reads possibly
/// sent. This is the backstop the signer's check runs ahead of.
#[test]
fn the_librarys_drain_minimum_refuses_only_after_signing() {
    let w = wallet_with_coins();
    let coins = plan(&w);
    let wallet = w.wallet;
    let f = fund_with(&w, &coins, move |s, coins, total, permit| async move {
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let credit_script = lib.shield_credit_script().await.unwrap();
        let signer = drain::drain_signer(&permit, &coins, total, 0, credit_script).unwrap();
        let (_, pw) = PwLib::open(&s, wallet).await.unwrap();
        (pw.fund_drain_with(total, &signer).await, permit)
    });
    let Err(FundError::NotSent(e)) = &f.result else {
        panic!("{f:?}");
    };
    assert!(e.to_string().contains("below the required minimum"), "{e}");
    assert_eq!(f.signatures, 3);
    assert_eq!(f.outcome, FlowOutcome::MaybeSent);
    assert_eq!(f.tracked, 0);
}
