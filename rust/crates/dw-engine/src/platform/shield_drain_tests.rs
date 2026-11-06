//! "Move mixed coins" to Shielded (0.2 MIX-SHIELD, `shield_drain.rs`) over
//! the scripted library: the plan's refusals, the drain of every coin into
//! one shield lock under a `Spend` grant, the signer that signs only the
//! planned coins, and the lock pending and resumed as any shield's.

use std::str::FromStr;
use std::sync::Arc;

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
        drain: coins,
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

/// A coin that came after the plan (its build would spend it) stops the
/// drain at the signer, before the lock is complete: nothing is funded,
/// the step reads not sent, and the next drain funds on the same step.
#[test]
fn a_coin_outside_the_plan_stops_the_drain_before_its_lock_exists() {
    let fx = Fixture::new(0);
    let lib = drainer(&fx, coins());
    *lib.drain_extra.lock().unwrap() = vec![coin(99, 100_001_000, true).path];
    let sum = total(&coins());
    assert_eq!(
        code(drain_with(&fx, &lib, spend_grant(&fx, sum))),
        "invalid_argument"
    );
    assert!(lib.funded.lock().unwrap().is_empty());
    assert!(
        fx.s.leases
            .signed_lock_txids(fx.wallet, &pool::shield_step(0))
            .is_empty()
    );

    // A second coin at a planned coin's address is one the plan lacks too.
    *lib.drain_extra.lock().unwrap() = vec![coins()[0].path.clone()];
    assert_eq!(
        code(drain_with(&fx, &lib, spend_grant(&fx, sum))),
        "invalid_argument"
    );

    lib.drain_extra.lock().unwrap().clear();
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    drain_with(&fx, &lib, spend_grant(&fx, sum)).unwrap();
    assert_eq!(lib.funded.lock().unwrap().len(), 1);
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
/// CoinJoin account's coins at its address paths, each confirmed or not
/// (an InstantSend lock is not a block), and
/// held when the user or dust protection locked it or a spend or lock
/// reserved it.
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
        ]
    );
}
