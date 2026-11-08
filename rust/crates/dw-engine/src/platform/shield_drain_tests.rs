//! "Move mixed coins" to Shielded (`shield_drain.rs`) over
//! the scripted library: the plan's refusals, the drain of every coin into
//! one shield lock under a `Spend` grant, the signer that signs only the
//! planned coins, and the lock pending and resumed as any shield's.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use dpp::balances::credits::CREDITS_PER_DUFF;
use key_wallet::bip32::DerivationPath;

use super::*;
use crate::platform::shielded::{MixedShieldQuote, ShieldOutcome};

/// The `i`th coin of CoinJoin account 0 (regtest), worth `value`.
fn coin(i: u32, value: u64, fully_mixed: bool) -> DrainCoin {
    DrainCoin {
        outpoint: OutPoint::new(Txid::from_byte_array([0x55; 32]), i),
        value,
        path: DerivationPath::from_str(&format!("m/9'/1'/4'/0'/0/{i}")).unwrap(),
        fully_mixed,
        confirmed: true,
        held: false,
    }
}

/// Three denominations, two fully mixed: 1.0 + 1.0 + 0.1 DASH.
fn coins() -> Vec<DrainCoin> {
    vec![
        coin(0, 100_001_000, true),
        coin(1, 100_001_000, true),
        coin(2, 10_000_100, false),
    ]
}

fn total(coins: &[DrainCoin]) -> u64 {
    coins.iter().map(|c| c.value).sum()
}

fn pool_fee() -> u64 {
    pool::shield_pool_fee(version(LATEST)).unwrap()
}

fn spend_grant(fx: &Fixture, max_duffs: u64) -> String {
    let purpose = dw_vault::GrantPurpose::Spend { max_duffs };
    (fx.s.vault)
        .authorize(purpose, Some(&fx.wallet.0), dw_vault::Credential::None)
        .unwrap()
        .id
}

/// A fake holding `coins` in its CoinJoin account, funding for `fx`.
fn drainer(fx: &Fixture, coins: Vec<DrainCoin>) -> Arc<FakeShielded> {
    let lib = Arc::new(FakeShielded {
        drain: Mutex::new(coins),
        ..FakeShielded::default()
    });
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
    lib
}

fn drain_with(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    grant: String,
) -> Result<ShieldOutcome, ShieldedError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        drain::drain(&s, wallet, &*lib, &grant).await
    })
}

fn quote_of(fx: &Fixture, lib: &Arc<FakeShielded>) -> Result<MixedShieldQuote, ShieldedError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        drain::quote(&s, wallet, &*lib).await
    })
}

/// The quote: every coin, its fee bound out of the lock, the pool fee out
/// of the credits, and the fully mixed part named.
#[test]
fn a_drain_quote_takes_every_coin_and_names_the_fully_mixed_part() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    let quote = quote_of(&fx, &lib).unwrap();
    let fee = crate::fees::asset_lock_fee_bound(3);
    assert_eq!(
        quote,
        MixedShieldQuote {
            inputs: 3,
            total_duffs: total(&coins()),
            fee_duffs: fee,
            duffs: total(&coins()) - fee,
            pool_fee_credits: pool_fee(),
            credits: (total(&coins()) - fee) * CREDITS_PER_DUFF - pool_fee(),
            fully_mixed_duffs: 200_002_000,
            not_fully_mixed_duffs: 10_000_100,
        }
    );
    assert!(
        lib.funded.lock().unwrap().is_empty(),
        "a quote funds nothing"
    );
}

/// The plan's refusals: a held coin (the library's drain would take it),
/// a coin not confirmed yet (the move takes them all), no coin, more coins
/// than one lock spends (combine them first), a lock below the pool's
/// minimum.
#[test]
fn a_drain_plan_refuses_a_held_or_unconfirmed_coin_too_many_coins_or_too_little() {
    let fee = pool_fee();
    let refusal = |coins: &[DrainCoin]| drain::plan_of(coins, fee).unwrap_err().code();
    let mut held = coins();
    held[1].held = true;
    assert_eq!(refusal(&held), "shielded.coins_locked");
    let mut pending = coins();
    pending[0].confirmed = false;
    pending[2].confirmed = false;
    assert_eq!(
        drain::plan_of(&pending, fee).unwrap_err(),
        ShieldedError::CoinsUnconfirmed { coins: 2 }
    );
    assert_eq!(refusal(&[]), "shielded.nothing_to_move");
    let many: Vec<DrainCoin> = (0..51).map(|i| coin(i, 100_001, true)).collect();
    assert_eq!(
        drain::plan_of(&many, fee).unwrap_err(),
        ShieldedError::TooManyInputs {
            inputs: 51,
            max: 50
        }
    );
    drain::plan_of(&many[..50], fee).unwrap();
    let min = fee / CREDITS_PER_DUFF + 1;
    let at = |v: u64| vec![coin(0, v + crate::fees::asset_lock_fee_bound(1), true)];
    assert_eq!(
        drain::plan_of(&at(min - 1), fee).unwrap_err(),
        ShieldedError::BelowMinimum { min }
    );
    drain::plan_of(&at(min), fee).unwrap();
}

/// The drain: one lock of every planned coin under a `Spend` grant of
/// their sum, applied; the lease charged the sum, fee included.
#[test]
fn a_drain_funds_one_lock_of_every_coin_and_applies_it() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let outcome = drain_with(&fx, &lib, spend_grant(&fx, total(&coins()))).unwrap();
    assert!(
        matches!(outcome.state, ShieldState::Applied { .. }),
        "{outcome:?}"
    );
    let inputs: Vec<OutPoint> = coins().iter().map(|c| c.outpoint).collect();
    assert_eq!(*lib.drained.lock().unwrap(), [inputs]);
    let lock = lib.funded.lock().unwrap()[0].outpoint;
    assert_eq!(outcome.txid, lock.txid.to_string());
    assert_eq!(*lib.applied.lock().unwrap(), [lock]);
    assert_eq!(*lib.settles.lock().unwrap(), [lock.txid]);
    // A shield step's own lock: the journal counts it this wallet's.
    let own = fx.s.leases.shield_lock_txids(fx.wallet).unwrap();
    assert!(own.contains(&lock.txid));
    // The funding ran under the wallet's claim, given back after it.
    assert_eq!(*lib.drain_claimed.lock().unwrap(), [true]);
    // The library drops a built lock the pool fee would leave nothing of.
    assert_eq!(
        *lib.drain_floors.lock().unwrap(),
        [drain::drain_floor(pool_fee())]
    );
    drop(fx.s.coinjoin.claim_sweep(fx.wallet).unwrap());
}

/// Another move of the account (a sweep, a consolidation, a resend) holds
/// the wallet's claim: the drain is refused before it reads coins, with
/// nothing funded and the grant unused, and runs once the move ended.
#[test]
fn a_drain_is_refused_while_another_move_of_the_coins_runs() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    let sum = total(&coins());
    let grant = spend_grant(&fx, sum);
    for resend in [false, true] {
        let claim = if resend {
            fx.s.coinjoin.claim_resend(fx.wallet)
        } else {
            fx.s.coinjoin.claim_sweep(fx.wallet)
        }
        .unwrap();
        assert_eq!(
            drain_with(&fx, &lib, grant.clone()).unwrap_err(),
            ShieldedError::MoveRunning
        );
        drop(claim);
    }
    assert!(lib.funded.lock().unwrap().is_empty());
    (fx.s.vault)
        .grant_purpose(&grant, dw_vault::GrantKind::Spend, Some(&fx.wallet.0))
        .unwrap();
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    drain_with(&fx, &lib, grant).unwrap();
    assert_eq!(lib.funded.lock().unwrap().len(), 1);
}

/// Refused with nothing funded and the grant unused: while the wallet
/// mixes, under a grant below the coins' sum, under a `PlatformOp` grant.
#[test]
fn a_drain_is_refused_while_mixing_or_past_its_grant_and_keeps_the_grant() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    let sum = total(&coins());

    fx.s.coinjoin.set_mixing_for_test(fx.wallet, true);
    assert_eq!(code(quote_of(&fx, &lib)), "shielded.mixing_active");
    let grant = spend_grant(&fx, sum);
    assert_eq!(
        code(drain_with(&fx, &lib, grant.clone())),
        "shielded.mixing_active"
    );
    fx.s.coinjoin.set_mixing_for_test(fx.wallet, false);

    let short = spend_grant(&fx, sum - 1);
    assert_eq!(
        drain_with(&fx, &lib, short.clone()).unwrap_err(),
        PlatformError::GrantExceeded {
            purpose: super::super::flows::BudgetPurpose::Funding,
            needed: sum,
            remaining: sum - 1,
        }
        .into()
    );
    assert_eq!(
        code(drain_with(&fx, &lib, shield_grant(&fx, 10 * sum))),
        "platform.grant_invalid"
    );
    assert!(lib.funded.lock().unwrap().is_empty());
    // Every refused grant is still unused.
    for g in [grant, short] {
        (fx.s.vault)
            .grant_purpose(&g, dw_vault::GrantKind::Spend, Some(&fx.wallet.0))
            .unwrap();
    }
}

/// A coin that came after the plan (the library's build spends it) stops
/// the drain at the signer, before any signature: nothing is funded, the
/// step reads not sent, and the next drain funds on the same step.
#[test]
fn a_coin_outside_the_plan_stops_the_drain_before_its_lock_exists() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    let with = |extra: DrainCoin| Some([coins(), vec![extra]].concat());
    *lib.drain_built.lock().unwrap() = with(coin(99, 100_001_000, true));
    let sum = total(&coins());
    assert_eq!(
        code(drain_with(&fx, &lib, spend_grant(&fx, sum))),
        "shielded.coins_changed"
    );
    assert!(lib.funded.lock().unwrap().is_empty());
    assert!(
        fx.s.leases
            .signed_lock_txids(fx.wallet, &pool::shield_step(0))
            .is_empty()
    );

    // A second coin at a planned coin's address is one the plan lacks too.
    *lib.drain_built.lock().unwrap() = with(DrainCoin {
        outpoint: OutPoint::new(Txid::from_byte_array([0x56; 32]), 0),
        ..coins()[0].clone()
    });
    assert_eq!(
        code(drain_with(&fx, &lib, spend_grant(&fx, sum))),
        "shielded.coins_changed"
    );

    *lib.drain_built.lock().unwrap() = None;
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    drain_with(&fx, &lib, spend_grant(&fx, sum)).unwrap();
    assert_eq!(lib.funded.lock().unwrap().len(), 1);
}

/// A drain refused before its first signature (`Cancelled`: nothing
/// signed, tracked or sent) leaves its coins to the user's next drain,
/// which builds the very same transaction: the same coins, the same
/// credit address (the library's peek, which a refused build does not
/// consume) and the same fee. That drain goes through, its lock recorded
/// under its own call on the same step: the refused call released no
/// signature, so the fence keeps nothing of it to match the lock against.
#[test]
fn a_drain_refused_before_signing_leaves_the_same_coins_to_the_next_drain() {
    use super::super::flows::DispatchState;
    let fx = Fixture::new(0);
    let all = [coins(), vec![coin(3, 100_001_000, true)]].concat();
    let lib = drainer(&fx, coins());
    // The library's build spends every coin, one that came after the plan too.
    *lib.drain_built.lock().unwrap() = Some(all.clone());
    let step = pool::shield_step(0);
    assert_eq!(
        code(drain_with(&fx, &lib, spend_grant(&fx, total(&coins())))),
        "shielded.coins_changed"
    );
    assert_ne!(
        fx.s.leases.funding_status(fx.wallet, &step),
        Some(DispatchState::MaybeSent)
    );
    assert!(fx.s.leases.signed_lock_txids(fx.wallet, &step).is_empty());
    assert!(lib.funded.lock().unwrap().is_empty());

    // Quoted again, the plan holds the coins the refused build spent.
    *lib.drain.lock().unwrap() = all.clone();
    *lib.drain_built.lock().unwrap() = None;
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let outcome = drain_with(&fx, &lib, spend_grant(&fx, total(&all))).unwrap();
    assert!(
        matches!(outcome.state, ShieldState::Applied { .. }),
        "{outcome:?}"
    );
    let lock = lib.funded.lock().unwrap()[0].outpoint;
    assert_eq!(
        lock.txid,
        fake_drain_build(&all).0.txid(),
        "the refused build's own transaction"
    );
    assert!(
        (fx.s.leases)
            .signed_lock_txids(fx.wallet, &step)
            .contains(&lock.txid)
    );
}

/// A drain whose lock signed and then failed (the library's floor, a
/// rejected broadcast, no verdict) may be out, as a shield's: its step
/// stays possibly sent, a stored lock's coins held for Repair, and the
/// next drain is refused until the step resolves.
#[test]
fn a_drain_failing_after_its_signatures_stays_possibly_sent() {
    use super::super::flows::DispatchState;
    for (end, error) in [
        (FundEnd::BelowFloor, "platform.broadcast_unknown"),
        (FundEnd::Rejected, "platform.broadcast_unknown"),
        (FundEnd::Unknown, "platform.broadcast_unknown"),
    ] {
        let fx = Fixture::new(0);
        let lib = drainer(&fx, coins());
        *lib.fund_end.lock().unwrap() = Some(end);
        let sum = total(&coins());
        assert_eq!(
            code(drain_with(&fx, &lib, spend_grant(&fx, sum))),
            error,
            "{end:?}"
        );
        assert_eq!(
            fx.s.leases.funding_status(fx.wallet, &pool::shield_step(0)),
            Some(DispatchState::MaybeSent),
            "{end:?}"
        );
        let held: std::collections::HashSet<OutPoint> = match end {
            FundEnd::BelowFloor => Default::default(),
            _ => coins().iter().map(|c| c.outpoint).collect(),
        };
        assert_eq!(
            fx.s.leases.unresolved_lock_inputs(fx.wallet),
            held,
            "{end:?}"
        );
        assert_eq!(
            code(drain_with(&fx, &lib, spend_grant(&fx, sum))),
            "platform.broadcast_unknown"
        );
        assert!(lib.funded.lock().unwrap().is_empty());
    }
}

/// A drain the network rejected after its signatures, end to end on a
/// session holding the CoinJoin coins: its step reads possibly sent and
/// refuses the next drain (the grant unused) until Repair's self-spend of
/// the lock's recorded CoinJoin inputs is ChainLocked. Then the step reads
/// not sent, nothing stays held, and the next drain funds its own lock on
/// the next step and applies it.
#[test]
fn a_drain_rejected_after_signing_is_repaired_and_the_next_drain_funds() {
    use super::super::flows::DispatchState;
    use crate::platform::lease::session_tests::fixture;
    use dw_vault::{Credential, GrantPurpose};
    const STEP: &str = "shield/0/funding";
    assert_eq!(pool::shield_step(0), STEP);
    let f = fixture(false);
    let (s, wallet) = (Arc::clone(&f.session), f.wallet);
    let a = f.credit_coinjoin(1, 100_001_000);
    let b = f.credit_coinjoin(2, 10_000_100);
    let lib = Arc::new(FakeShielded::default());
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&s), wallet));
    // The library's drain reads the wallet's own CoinJoin coins.
    let read_coins = || {
        let inner = Arc::clone(&s);
        let coins = (f.engine).block_on(s.on_runtime(async move {
            let fence = inner.coin_fence(wallet).await?;
            Ok(drain::read_drain_coins(&inner, &fence).await.unwrap())
        }));
        *lib.drain.lock().unwrap() = coins.unwrap();
        let outpoints: std::collections::HashSet<OutPoint> = (lib.drain.lock().unwrap().iter())
            .map(|c| c.outpoint)
            .collect();
        outpoints
    };
    let spend = |max_duffs| f.grant(GrantPurpose::Spend { max_duffs }, Credential::None);
    let drain = |grant: String| {
        let (s, lib) = (Arc::clone(&s), Arc::clone(&lib));
        (f.engine).block_on(async move { drain::drain(&s, wallet, &*lib, &grant).await })
    };
    let status = |step: &str| s.leases.funding_status(wallet, step);

    assert_eq!(read_coins(), [a, b].into());
    let sum = 100_001_000 + 10_000_100;
    *lib.fund_end.lock().unwrap() = Some(FundEnd::Rejected);
    assert_eq!(code(drain(spend(sum))), "platform.broadcast_unknown");
    assert_eq!(status(STEP), Some(DispatchState::MaybeSent));
    assert_eq!(s.leases.unresolved_lock_inputs(wallet), [a, b].into());
    let refused = spend(sum);
    assert_eq!(code(drain(refused.clone())), "platform.broadcast_unknown");
    (s.vault())
        .grant_purpose(&refused, dw_vault::GrantKind::Spend, Some(&wallet.0))
        .unwrap();

    // Tools ▸ Repair finds the step through the listing, priced.
    let stuck = || {
        (f.engine)
            .block_on(s.dashpay(wallet).stuck_shield_fundings())
            .unwrap()
    };
    let [listed] = <[_; 1]>::try_from(stuck()).unwrap();
    assert_eq!(
        (listed.step.as_str(), listed.state),
        (STEP, DispatchState::MaybeSent)
    );
    let cost = listed.repair.unwrap();
    assert_eq!((cost.mixed, cost.inputs, cost.total_duffs), (true, 2, sum));
    let tx = f.repair(Box::leak(listed.step.into_boxed_str()));
    let spent: std::collections::HashSet<OutPoint> =
        tx.input.iter().map(|i| i.previous_output).collect();
    assert_eq!(spent, [a, b].into());
    let paid: u64 = tx.output.iter().map(|o| o.value).sum();
    assert_eq!(
        cost.fee_duffs,
        sum - paid,
        "the listed fee is the one Repair paid"
    );
    assert_eq!(status(STEP), Some(DispatchState::NotSent));
    assert!(s.leases.unresolved_lock_inputs(wallet).is_empty());
    assert!(stuck().is_empty(), "a resolved step leaves the listing");

    // key-wallet drops the coins the self-spend took; new mixed coins come.
    let inner = Arc::clone(&s);
    (f.engine)
        .block_on(s.on_runtime(async move {
            let w = inner.wallet(&wallet).await?;
            let mut state = w.state_mut().await;
            let account = (state.core_wallet.accounts.coinjoin_accounts.get_mut(&0)).unwrap();
            assert!(account.utxos.remove(&a).is_some() && account.utxos.remove(&b).is_some());
            Ok(())
        }))
        .unwrap();
    let c = f.credit_coinjoin(3, 100_001_000);
    assert_eq!(read_coins(), [c].into());
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let outcome = drain(spend(100_001_000)).unwrap();
    assert!(
        matches!(outcome.state, ShieldState::Applied { .. }),
        "{outcome:?}"
    );
    assert_eq!(*lib.drained.lock().unwrap(), [vec![c]]);
    let lock = lib.funded.lock().unwrap()[0].outpoint;
    assert_eq!(outcome.txid, lock.txid.to_string());
    assert!(
        (s.leases)
            .signed_lock_txids(wallet, &pool::shield_step(1))
            .contains(&lock.txid)
    );
    assert_eq!(status(STEP), Some(DispatchState::NotSent));
}

/// Tools ▸ Repair's listing of the moves into Shielded that may be out:
/// a drain reading `MaybeSent`, priced by Repair's own draft, and nothing
/// that is applied, waiting on its pending lock, or unsent.
#[test]
fn the_repair_listing_holds_only_the_possibly_sent_moves() {
    use super::super::flows::DispatchState;
    use crate::platform::lease::session_tests::fixture;
    use dw_vault::{Credential, GrantPurpose};
    let f = fixture(false);
    let (s, wallet) = (Arc::clone(&f.session), f.wallet);
    // Each drain that signed spends coins of its own, as on the chain,
    // where an applied or pending lock has spent its coins.
    let a = f.credit_coinjoin(1, 100_001_000);
    let b = f.credit_coinjoin(2, 100_001_000);
    let c = f.credit_coinjoin(3, 100_001_000);
    let d = f.credit_coinjoin(4, 10_000_100);
    let lib = Arc::new(FakeShielded::default());
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&s), wallet));
    let plan = |of: &[OutPoint]| {
        let inner = Arc::clone(&s);
        let coins = (f.engine).block_on(s.on_runtime(async move {
            let fence = inner.coin_fence(wallet).await?;
            Ok(drain::read_drain_coins(&inner, &fence).await.unwrap())
        }));
        let coins: Vec<DrainCoin> = (coins.unwrap().into_iter())
            .filter(|c| of.contains(&c.outpoint))
            .collect();
        assert_eq!(coins.len(), of.len());
        let sum = total(&coins);
        *lib.drain.lock().unwrap() = coins;
        sum
    };
    let spend = |max_duffs| f.grant(GrantPurpose::Spend { max_duffs }, Credential::None);
    let op = |max_duffs| {
        f.grant(
            GrantPurpose::PlatformOp {
                max_duffs,
                max_credits: 0,
            },
            Credential::None,
        )
    };
    let drain = |grant: String| {
        let (s, lib) = (Arc::clone(&s), Arc::clone(&lib));
        (f.engine).block_on(async move { drain::drain(&s, wallet, &*lib, &grant).await })
    };
    let stuck = || {
        (f.engine)
            .block_on(s.dashpay(wallet).stuck_shield_fundings())
            .unwrap()
    };
    let steps = || -> Vec<(String, DispatchState)> {
        stuck().into_iter().map(|l| (l.step, l.state)).collect()
    };

    // Applied.
    let sum = plan(&[a]);
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    drain(spend(sum)).unwrap();
    assert_eq!(steps(), []);
    // Refused before its signatures: unsent.
    let sum = plan(&[b]);
    *lib.drain_built.lock().unwrap() = Some(
        [
            lib.drain.lock().unwrap().clone(),
            vec![coin(99, 100_001_000, true)],
        ]
        .concat(),
    );
    assert_eq!(code(drain(spend(sum))), "shielded.coins_changed");
    *lib.drain_built.lock().unwrap() = None;
    assert_eq!(steps(), []);
    // Its apply failed: the lock pending, waiting for "Check again".
    *lib.applies.lock().unwrap() = Some(Err(PlatformError::Unavailable));
    drain(spend(sum)).unwrap();
    assert_eq!(steps(), []);
    let lock = lib.funded.lock().unwrap()[1].outpoint;
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let (txid, grant) = (lock.txid.to_string(), op(1));
    let (rs, rlib) = (Arc::clone(&s), Arc::clone(&lib));
    (f.engine)
        .block_on(async move { pool::resume(&rs, wallet, &*rlib, &txid, &grant).await })
        .unwrap();

    // A drain rejected after its signatures: listed, priced on its coins.
    let step = pool::shield_step(2);
    let sum = plan(&[c, d]);
    *lib.fund_end.lock().unwrap() = Some(FundEnd::Rejected);
    assert_eq!(code(drain(spend(sum))), "platform.broadcast_unknown");
    let [listed] = <[_; 1]>::try_from(stuck()).unwrap();
    assert_eq!(
        (&listed.step, listed.state),
        (&step, DispatchState::MaybeSent)
    );
    let cost = listed.repair.unwrap();
    assert_eq!((cost.mixed, cost.inputs, cost.total_duffs), (true, 2, sum));
    assert!(cost.fee_duffs > 0 && cost.fee_duffs < 10_000, "{cost:?}");
    let tx = f.repair(Box::leak(listed.step.into_boxed_str()));
    let spent: std::collections::HashSet<OutPoint> =
        tx.input.iter().map(|i| i.previous_output).collect();
    assert_eq!(spent, [c, d].into());
    assert_eq!(steps(), []);
}

/// A shield (not a drain) rejected after its signature is listed too. Its
/// lock spent coins the wallet does not hold, so Repair refuses it, and
/// the listing carries that refusal in place of a price.
#[test]
fn the_repair_listing_holds_a_possibly_sent_shield_with_repairs_refusal() {
    use super::super::flows::DispatchState;
    use crate::platform::lease::session_tests::fixture;
    use dw_vault::{Credential, GrantPurpose};
    let f = fixture(false);
    let (s, wallet) = (Arc::clone(&f.session), f.wallet);
    f.credit(9, 50_000_000);
    let lib = Arc::new(FakeShielded::default());
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&s), wallet));
    *lib.fund_end.lock().unwrap() = Some(FundEnd::Rejected);
    let grant = f.grant(
        GrantPurpose::PlatformOp {
            max_duffs: 2_000_000,
            max_credits: 0,
        },
        Credential::None,
    );
    let (rs, rlib) = (Arc::clone(&s), Arc::clone(&lib));
    let shielded = (f.engine)
        .block_on(async move { pool::shield(&rs, wallet, &*rlib, 1_000_000, &grant).await });
    assert_eq!(code(shielded), "platform.broadcast_unknown");
    let stuck = (f.engine).block_on(s.dashpay(wallet).stuck_shield_fundings());
    let [listed] = <[_; 1]>::try_from(stuck.unwrap()).unwrap();
    assert_eq!(
        (listed.step.as_str(), listed.state),
        (pool::shield_step(0).as_str(), DispatchState::MaybeSent)
    );
    let refused = listed.repair.unwrap_err();
    assert_eq!(refused.code(), "stuck_funding", "{refused:?}");
}

/// A move into Shielded whose lock was sent and got no proof within the
/// wait is pending, not stuck: `pending_shields` lists it ("Check again"
/// applies it) and Repair's listing does not, before and after a restart.
#[test]
fn a_lock_waiting_for_its_proof_is_pending_not_stuck() {
    use super::super::flows::DispatchState;
    use crate::platform::lease::session_tests::{Fixture, fixture};
    use dw_vault::{Credential, GrantPurpose};
    let mut f = fixture(false);
    let wallet = f.wallet;
    let lib = Arc::new(FakeShielded {
        real_rows: true,
        ..FakeShielded::default()
    });
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&f.session), wallet));
    *lib.fund_end.lock().unwrap() = Some(FundEnd::ProofTimeout);
    let grant = f.grant(
        GrantPurpose::PlatformOp {
            max_duffs: 2_000_000,
            max_credits: 0,
        },
        Credential::None,
    );
    let (s, rlib) = (Arc::clone(&f.session), Arc::clone(&lib));
    let outcome = (f.engine)
        .block_on(async move { pool::shield(&s, wallet, &*rlib, 1_000_000, &grant).await })
        .unwrap();
    assert_eq!(outcome.state, ShieldState::ProofWaiting);
    let step = pool::shield_step(0);
    let pending_not_stuck = |f: &Fixture| {
        let stuck = (f.engine).block_on(f.session.dashpay(wallet).stuck_shield_fundings());
        assert!(stuck.as_ref().is_ok_and(|s| s.is_empty()), "{stuck:?}");
        assert_eq!(
            f.session.leases.funding_status(wallet, &step),
            Some(DispatchState::WillBeSent)
        );
        let l = Arc::clone(&lib);
        let pending =
            (f.engine).block_on(async move { pool::status(&*l).await.unwrap().pending_shields });
        let pending: Vec<_> = (pending.iter())
            .map(|p| (p.txid.as_str(), p.proved))
            .collect();
        assert_eq!(pending, [(outcome.txid.as_str(), false)]);
    };
    pending_not_stuck(&f);
    f.reopen();
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&f.session), wallet));
    pending_not_stuck(&f);
}

/// A drain whose apply failed leaves its lock pending, as a shield's: it
/// blocks a new drain (no second lock of the coins), and "Check again"
/// applies the same outpoint, never a new funding.
#[test]
fn a_pending_drain_lock_blocks_a_new_drain_and_resumes_as_a_shield() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    *lib.applies.lock().unwrap() = Some(Err(PlatformError::Unavailable));
    let sum = total(&coins());
    let outcome = drain_with(&fx, &lib, spend_grant(&fx, sum)).unwrap();
    assert_eq!(
        outcome.state,
        ShieldState::NotApplied {
            error: "platform.unavailable".into()
        }
    );
    let lock = lib.funded.lock().unwrap()[0].outpoint;
    assert_eq!(
        drain_with(&fx, &lib, spend_grant(&fx, sum)).unwrap_err(),
        ShieldedError::ShieldPending {
            txid: lock.txid.to_string()
        }
    );
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let (wallet, txid, grant) = (fx.wallet, lock.txid.to_string(), shield_grant(&fx, 1));
    let state = run(&fx, &lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid, &grant).await
    })
    .unwrap()
    .state;
    assert!(matches!(state, ShieldState::Applied { .. }), "{state:?}");
    assert_eq!(*lib.applied.lock().unwrap(), [lock, lock]);
    assert_eq!(lib.drained.lock().unwrap().len(), 1, "no other lock");
}

/// The real library's coins of the drain ([`drain::drain_coins_of`]): the
/// CoinJoin account's coins, each confirmed or not (an InstantSend lock is
/// not a block, an immature coinbase output is not final), and held when
/// the user or dust protection locked it, a spend or lock reserved it, or
/// it lies off the account's address paths (the drain would take it, and
/// its signer could not sign it).
#[test]
fn the_drain_reads_the_coinjoin_accounts_coins_and_what_holds_them() {
    use crate::coinjoin::{CoinJoinBalances, MixView, MixViewCoin};
    use crate::coins::WalletCoin;
    use dashcore::{Address, TxOut};

    let mixed = |n: u8, coinjoin_account: bool, path: &str| {
        let address = Address::from_str("yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n")
            .unwrap()
            .assume_checked();
        let txout = TxOut {
            value: 100_001_000 + u64::from(n),
            script_pubkey: address.script_pubkey(),
        };
        let outpoint = OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let mut utxo = key_wallet::Utxo::new(outpoint, txout, address, 100, false);
        utxo.is_confirmed = true;
        MixViewCoin {
            coin: WalletCoin {
                utxo,
                send_account: !coinjoin_account,
                coinjoin_account,
                is_change: false,
                block_time: None,
                chain_locked: false,
                foreign_incoming: true,
                user_locked: false,
                dust_locked: false,
                reserved: false,
                lock_input: false,
                unresolved_lock: false,
            },
            path: Some(path.parse().unwrap()),
            rounds: 4,
            fully_mixed: n.is_multiple_of(2),
        }
    };
    let cj = "m/9'/1'/4'/0'/0/3";
    let mut pending = mixed(3, true, cj);
    pending.coin.utxo.is_confirmed = false;
    let mut locked_ix = mixed(4, true, "m/9'/1'/4'/0'/1/0");
    locked_ix.coin.utxo.is_confirmed = false;
    locked_ix.coin.utxo.is_instantlocked = true;
    let mut user = mixed(5, true, cj);
    user.coin.user_locked = true;
    let mut reserved = mixed(6, true, cj);
    reserved.coin.reserved = true;
    let mut coinbase = mixed(8, true, cj);
    coinbase.coin.utxo.is_coinbase = true;
    coinbase.coin.utxo.height = 150;
    let mut mature = mixed(9, true, cj);
    mature.coin.utxo.is_coinbase = true;
    let view = MixView {
        height: 200,
        coins: vec![
            mixed(1, true, cj),
            mixed(2, false, "m/44'/1'/0'/0/1"),
            pending,
            locked_ix,
            user,
            reserved,
            // In the account, off its address paths.
            mixed(7, true, "m/9'/1'/5'/5'/0"),
            coinbase,
            mature,
        ],
        balances: CoinJoinBalances::default(),
        total: 0,
        average_rounds: 0.0,
    };
    let got: Vec<(u8, bool, bool, bool)> = drain::drain_coins_of(view, dashcore::Network::Testnet)
        .iter()
        .map(|c| {
            let n = c.outpoint.txid.to_byte_array()[0];
            (n, c.fully_mixed, c.confirmed, c.held)
        })
        .collect();
    assert_eq!(
        got,
        [
            (1, false, true, false),
            (3, false, false, false),
            (4, true, false, false),
            (5, false, true, true),
            (6, true, true, true),
            (7, false, true, true),
            (8, true, false, false),
            (9, false, true, false),
        ]
    );
}
