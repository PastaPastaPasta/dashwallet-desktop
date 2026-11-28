//! Core Max to the Platform address (`platform_fund(All)`) over the
//! scripted library: the quote and the fund agree, a build that is not the
//! plan is `credits.coins_changed` with nothing stored, the refusals before
//! signing, and the lock pending and resumed on its own outpoint.

use super::super::core_max::CoreCoins;
use super::super::credits::{CoreMaxArrivalBlock, CoreMaxShort};
use super::super::shield_drain::DrainCoin;
use super::*;

fn bip44(i: u32, value: u64) -> DrainCoin {
    DrainCoin {
        outpoint: OutPoint::new(Txid::from_byte_array([0x45; 32]), i),
        value,
        path: DerivationPath::from_str(&format!("m/44'/1'/0'/0/{i}")).unwrap(),
        fully_mixed: false,
        confirmed: true,
        held: false,
    }
}

fn account() -> CoreCoins {
    CoreCoins {
        coins: vec![bip44(0, 400_000_000), bip44(1, 60_000_000)],
        left_duffs: 3_000,
        ..CoreCoins::default()
    }
}

fn late(i: u32, value: u64) -> DrainCoin {
    DrainCoin {
        confirmed: false,
        ..bip44(i, value)
    }
}

const TOTAL: u64 = 460_000_000;

fn maxer(fx: &Fixture, read: CoreCoins) -> Arc<FakeFund> {
    let lib = FakeFund::new(fx);
    *lib.core.lock().unwrap() = read;
    Arc::new(lib)
}

fn max_quote(fx: &Fixture, lib: &Arc<FakeFund>) -> Result<PlatformFundQuote, CreditsError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        fund::quote(&s, wallet, &*lib, FundAmount::All).await
    })
}

fn max_fund(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    max_duffs: u64,
) -> (Result<PlatformFundOutcome, CreditsError>, String) {
    let coin_set = super::super::core_max::coin_set(&lib.core.lock().unwrap().coins);
    fund_quoted(fx, lib, max_duffs, coin_set)
}

fn fund_quoted(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    max_duffs: u64,
    coin_set: [u8; 32],
) -> (Result<PlatformFundOutcome, CreditsError>, String) {
    let (wallet, grant) = (fx.wallet, grant(fx, max_duffs, 0));
    let g = grant.clone();
    let out = run(fx, lib, move |s, lib| async move {
        fund::fund(&s, wallet, &*lib, FundAmount::QuotedAll { coin_set }, &g).await
    });
    (out, grant)
}

fn unused(fx: &Fixture, grant: &str) {
    (fx.s.vault)
        .grant_purpose(grant, dw_vault::GrantKind::PlatformOp, Some(&fx.wallet.0))
        .unwrap();
}

/// Nothing tracked or signed on the fund's step, which is not left
/// possibly sent: no lock exists to record in the activity.
fn nothing_stored(fx: &Fixture, lib: &Arc<FakeFund>) {
    use super::super::flows::DispatchState;
    let step = fund::fund_step(0);
    assert!(lib.tracked.lock().unwrap().is_empty(), "no lock tracked");
    assert!(lib.core_drained.lock().unwrap().is_empty());
    assert!((fx.s.leases.signed_lock_txids(fx.wallet, &step)).is_empty());
    assert_ne!(
        fx.s.leases.funding_status(fx.wallet, &step),
        Some(DispatchState::MaybeSent)
    );
}

/// The quote drains the account, the fee out of the lock; the fund under
/// exactly its grant spends those coins in one lock and applies it.
#[test]
fn core_max_fund_quote_and_execute_agree() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let quote = max_quote(&fx, &lib).unwrap();
    let lock_fee = crate::fees::asset_lock_fee_bound(2);
    assert_eq!(
        (quote.total_duffs, quote.fee_duffs, quote.duffs),
        (TOTAL, lock_fee, TOTAL - lock_fee)
    );
    assert_eq!(quote.credits, (TOTAL - lock_fee) * 1_000 - fee());
    assert_eq!((quote.grant.max_duffs, quote.grant.max_credits), (TOTAL, 0));
    let max = quote.max.unwrap();
    assert_eq!(
        (max.inputs, max.left_duffs, max.arriving_duffs),
        (2, 3_000, 0)
    );

    lib.answer(Answer::Done(None));
    let (out, _) = fund_quoted(&fx, &lib, quote.grant.max_duffs, max.coin_set);
    let out = out.unwrap();
    assert!(
        matches!(out.state, PlatformFundState::Applied { .. }),
        "{out:?}"
    );
    let planned: Vec<OutPoint> = account().coins.iter().map(|c| c.outpoint).collect();
    assert_eq!(*lib.core_drained.lock().unwrap(), [planned]);
    let outpoint = lib.outpoint(0);
    assert_eq!(out.txid, outpoint.txid.to_string());
    assert_eq!(lib.applied(), [outpoint]);
    assert_eq!(lib.tracked.lock().unwrap()[0].amount, TOTAL - lock_fee);
    assert_eq!(
        *lib.core_fenced.lock().unwrap(),
        [true],
        "built and signed under the fence"
    );
    drop(fx.s.coinjoin.claim_sweep(fx.wallet).unwrap());
}

/// A changed input set is `coins_changed` under the quote's grant,
/// before the lease: nothing stored, the grant unused; a quote of the coins
/// as they are funds.
#[test]
fn core_max_fund_refuses_coins_changed_since_the_quote() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let quoted = max_quote(&fx, &lib).unwrap();
    let spent = CoreCoins {
        coins: account().coins[..1].to_vec(),
        ..account()
    };
    let mut swapped = account();
    swapped.coins[1] = bip44(9, swapped.coins[1].value - 1);
    let mut same_sum = account();
    let value = same_sum.coins.pop().unwrap().value;
    same_sum
        .coins
        .extend([bip44(8, value / 2), bip44(9, value / 2)]);
    let mut same_count = account();
    same_count.coins[0] = bip44(9, same_count.coins[0].value);
    let mut received = account();
    received.coins.push(bip44(10, 1_000));
    for read in [spent, swapped, same_sum, same_count, received] {
        *lib.core.lock().unwrap() = read;
        let (out, g) = fund_quoted(
            &fx,
            &lib,
            quoted.grant.max_duffs,
            quoted.max.unwrap().coin_set,
        );
        assert_eq!(out.unwrap_err(), CreditsError::CoinsChanged);
        unused(&fx, &g);
        nothing_stored(&fx, &lib);
    }
    let requoted = max_quote(&fx, &lib).unwrap().grant.max_duffs;
    lib.answer(Answer::Done(None));
    max_fund(&fx, &lib, requoted).0.unwrap();
    assert_eq!(lib.core_drained.lock().unwrap().len(), 1);
}

/// A build that is not the plan is refused unsigned: `coins_changed`,
/// nothing tracked, recorded or sent, and the next Max funds.
#[test]
fn core_max_fund_signs_exactly_the_planned_coins() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    *lib.core_built.lock().unwrap() = Some([account().coins, vec![bip44(7, 5_000)]].concat());
    let (out, _) = max_fund(&fx, &lib, TOTAL);
    assert_eq!(out.unwrap_err(), CreditsError::CoinsChanged);
    nothing_stored(&fx, &lib);
    *lib.core_built.lock().unwrap() = None;
    lib.answer(Answer::Done(None));
    max_fund(&fx, &lib, TOTAL).0.unwrap();
    assert_eq!(lib.core_drained.lock().unwrap().len(), 1);
}

/// The refusals before signing, each leaving the grant unused and nothing
/// stored: a held coin, more than 50 coins, a grant below the coins' sum,
/// the wallet mixing, another move of its coins running.
#[test]
fn core_max_fund_refuses_before_signing() {
    let fx = Fixture::new(0);
    let mut held = account();
    held.coins[0].held = true;
    let lib = maxer(&fx, held);
    assert_eq!(code(max_quote(&fx, &lib)), "credits.coins_locked");
    let (out, g) = max_fund(&fx, &lib, TOTAL);
    assert_eq!(code(out), "credits.coins_locked");
    unused(&fx, &g);
    nothing_stored(&fx, &lib);

    let many = CoreCoins {
        coins: (0..51).map(|i| bip44(i, 1_000_000)).collect(),
        ..CoreCoins::default()
    };
    let lib = maxer(&fx, many);
    let refused = CreditsError::TooManyInputs {
        inputs: 51,
        max: 50,
    };
    assert_eq!(max_quote(&fx, &lib).unwrap_err(), refused);
    let (out, g) = max_fund(&fx, &lib, 51_000_000);
    assert_eq!(out.unwrap_err(), refused);
    unused(&fx, &g);

    let lib = maxer(&fx, account());
    let (out, g) = max_fund(&fx, &lib, TOTAL - 1);
    assert_eq!(code(out), "platform.grant_exceeded");
    unused(&fx, &g);

    fx.s.coinjoin.set_mixing_for_test(fx.wallet, true);
    assert_eq!(code(max_quote(&fx, &lib)), "credits.mixing_active");
    let (out, g) = max_fund(&fx, &lib, TOTAL);
    assert_eq!(code(out), "credits.mixing_active");
    unused(&fx, &g);
    fx.s.coinjoin.set_mixing_for_test(fx.wallet, false);
    let sweep = fx.s.coinjoin.claim_sweep(fx.wallet).unwrap();
    assert_eq!(code(max_quote(&fx, &lib)), "credits.move_running");
    let (out, g) = max_fund(&fx, &lib, TOTAL);
    assert_eq!(code(out), "credits.move_running");
    unused(&fx, &g);
    drop(sweep);
    nothing_stored(&fx, &lib);
}

/// A lock below the fund's floor is refused before signing, with what Max
/// would move, what the other accounts hold and what is still arriving.
#[test]
fn core_max_fund_refuses_below_the_floor() {
    let fx = Fixture::new(0);
    let floor = fee() / 1_000 + 1;
    let fee = crate::fees::asset_lock_fee_bound(1);
    let read = CoreCoins {
        coins: vec![bip44(0, floor + fee - 1)],
        left_duffs: 3_000,
        arriving_duffs: 8_000,
        arriving: vec![late(1, 8_000)],
        ..CoreCoins::default()
    };
    let lib = maxer(&fx, read);
    let below = CreditsError::BelowMinimum {
        min: floor,
        max: Some(CoreMaxShort {
            duffs: floor - 1,
            left_duffs: 3_000,
            arriving_duffs: 8_000,
            arriving_clears: true,
            arriving_block: None,
            other_arriving_duffs: 0,
        }),
    };
    assert_eq!(max_quote(&fx, &lib).unwrap_err(), below);
    let (out, g) = max_fund(&fx, &lib, floor + fee - 1);
    assert_eq!(out.unwrap_err(), below);
    unused(&fx, &g);
    nothing_stored(&fx, &lib);
}

/// As the shield's: below the fund's floor, whether Max would clear it once
/// every arriving coin is final, by the same plan over all the coins.
#[test]
fn core_max_fund_says_whether_the_arriving_coins_would_clear() {
    let fx = Fixture::new(0);
    let floor = fee() / 1_000 + 1;
    let readiness = |coins: Vec<DrainCoin>, arriving: Vec<DrainCoin>| {
        let read = CoreCoins {
            coins,
            left_duffs: 0,
            arriving_duffs: arriving.iter().map(|c| c.value).sum(),
            arriving,
            ..CoreCoins::default()
        };
        match max_quote(&fx, &maxer(&fx, read)).unwrap_err() {
            CreditsError::BelowMinimum { max: Some(m), .. } => {
                (m.arriving_clears, m.arriving_block)
            }
            refused => panic!("{refused:?}"),
        }
    };
    let clears = |coins, arriving| readiness(coins, arriving).0;
    let at = |n: usize| floor + crate::fees::asset_lock_fee_bound(n);
    assert!(!clears(vec![], vec![]));
    assert!(!clears(vec![bip44(0, 1_000)], vec![]));
    assert!(!clears(vec![], vec![late(0, 5_000)]));
    assert!(clears(vec![], vec![late(0, at(1))]));
    assert!(!clears(vec![], vec![late(0, at(1) - 1)]));
    assert!(clears(vec![bip44(0, 1_000)], vec![late(1, at(2) - 1_000)]));
    assert!(!clears(vec![bip44(0, 1_000)], vec![late(1, at(2) - 1_001)]));
    let locked = DrainCoin {
        held: true,
        ..late(0, 10 * at(1))
    };
    assert_eq!(
        readiness(vec![], vec![locked]),
        (false, Some(CoreMaxArrivalBlock::Held))
    );
    let many: Vec<_> = (1..=50).map(|i| late(i, 1_000_000)).collect();
    assert_eq!(
        readiness(vec![bip44(0, 1_000)], many.clone()),
        (false, Some(CoreMaxArrivalBlock::InputCap))
    );
    assert!(clears(vec![bip44(0, 1_000)], many[..49].to_vec()));
}

/// A Max lock signed and out but not applied pends; after a restart (a new
/// library over the same tracked rows) a new Max is refused on it and the
/// resume applies that same outpoint, never a new lock.
#[test]
fn kill_after_drain_lock_signed_resumes_same_outpoint() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    lib.refuse(Answer::Failed);
    let out = max_fund(&fx, &lib, TOTAL).0.unwrap();
    assert!(
        matches!(out.state, PlatformFundState::NotApplied { .. }),
        "{out:?}"
    );
    let outpoint = lib.outpoint(0);

    let restarted = maxer(&fx, account());
    *restarted.tracked.lock().unwrap() = lib.tracked.lock().unwrap().clone();
    let lib = restarted;
    assert_eq!(
        pending_of(&fx, &lib),
        [(outpoint.txid.to_string(), true, false)]
    );
    assert_eq!(
        max_fund(&fx, &lib, TOTAL).0.unwrap_err(),
        CreditsError::FundPending {
            txid: outpoint.txid.to_string()
        }
    );
    lib.answer(Answer::Done(None));
    let state = resume_of(&fx, &lib, outpoint).unwrap();
    assert!(
        matches!(state, PlatformFundState::Applied { .. }),
        "{state:?}"
    );
    assert_eq!(lib.applied(), [outpoint]);
    assert!(lib.core_drained.lock().unwrap().is_empty(), "no new lock");
    assert_eq!(lib.funds.load(Ordering::SeqCst), 0);
}
