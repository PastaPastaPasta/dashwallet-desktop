//! Core Max to Shielded (`core_max.rs` through `shield_from_core(All)`)
//! over the scripted library: the plan of BIP44 account 0 with the fee out
//! of the lock, its refusals before signing, the drain signer that signs
//! exactly the planned build, and the lock pending and resumed as any
//! shield's. The read of the wallet's coins is `coins_of`'s test.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use dpp::balances::credits::CREDITS_PER_DUFF;
use key_wallet::bip32::DerivationPath;

use super::*;
use crate::platform::core_max::{self, CoreCoins};
use crate::platform::credits::CoreMaxShort;
use crate::platform::flows::GrantRequest;
use crate::platform::shielded::{ShieldOutcome, ShieldQuote};

/// The `i`th coin of BIP44 account 0 (regtest), worth `value`.
pub(super) fn bip44(i: u32, value: u64) -> DrainCoin {
    DrainCoin {
        outpoint: OutPoint::new(Txid::from_byte_array([0x44; 32]), i),
        value,
        path: DerivationPath::from_str(&format!("m/44'/1'/0'/0/{i}")).unwrap(),
        fully_mixed: false,
        confirmed: true,
        held: false,
    }
}

/// Three coins, with what stays behind.
pub(super) fn account() -> CoreCoins {
    CoreCoins {
        coins: vec![
            bip44(0, 300_000_000),
            bip44(1, 50_000_000),
            bip44(2, 2_000_000),
        ],
        left_duffs: 7_000_000,
        arriving_duffs: 1_500_000,
    }
}

fn sum(coins: &[DrainCoin]) -> u64 {
    coins.iter().map(|c| c.value).sum()
}

fn pool_fee() -> u64 {
    pool::shield_pool_fee(version(LATEST)).unwrap()
}

/// A fake holding `read` as BIP44 account 0, funding for `fx`.
fn maxer(fx: &Fixture, read: CoreCoins) -> Arc<FakeShielded> {
    let lib = Arc::new(FakeShielded {
        core: Mutex::new(read),
        ..FakeShielded::default()
    });
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
    lib
}

fn max_of(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    grant: String,
) -> Result<ShieldOutcome, ShieldedError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        pool::shield(&s, wallet, &*lib, FundAmount::All, &grant).await
    })
}

fn quote_of(fx: &Fixture, lib: &Arc<FakeShielded>) -> Result<ShieldQuote, ShieldedError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        pool::shield_max_quote(&s, wallet, &*lib).await
    })
}

/// The grant is still unused: nothing redeemed it.
fn unused(fx: &Fixture, grant: &str) {
    (fx.s.vault)
        .grant_purpose(grant, dw_vault::GrantKind::PlatformOp, Some(&fx.wallet.0))
        .unwrap();
}

/// The quote drains every planned coin, the fee out of the lock, and
/// names what stays; the shield under exactly its grant spends those
/// coins in one lock and applies it, under the wallet's CoinJoin claim.
#[test]
fn core_max_quote_and_execute_agree() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let quote = quote_of(&fx, &lib).unwrap();
    let total = sum(&account().coins);
    let fee = crate::fees::asset_lock_fee_bound(3);
    assert_eq!(
        quote,
        ShieldQuote {
            duffs: total - fee,
            fee_duffs: fee,
            total_duffs: total,
            pool_fee_credits: pool_fee(),
            credits: (total - fee) * CREDITS_PER_DUFF - pool_fee(),
            grant: GrantRequest {
                max_duffs: total,
                max_credits: 0,
            },
            max: Some(core_max_quote(3)),
        }
    );
    assert!(
        lib.funded.lock().unwrap().is_empty(),
        "a quote funds nothing"
    );
    drop(fx.s.coinjoin.claim_sweep(fx.wallet).unwrap());

    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let outcome = max_of(&fx, &lib, shield_grant(&fx, quote.grant.max_duffs)).unwrap();
    assert!(
        matches!(outcome.state, ShieldState::Applied { .. }),
        "{outcome:?}"
    );
    let planned: Vec<OutPoint> = account().coins.iter().map(|c| c.outpoint).collect();
    assert_eq!(*lib.drained.lock().unwrap(), [planned]);
    let lock = lib.funded.lock().unwrap()[0].outpoint;
    assert_eq!(outcome.txid, lock.txid.to_string());
    assert_eq!(*lib.applied.lock().unwrap(), [lock]);
    assert!(
        (fx.s
            .leases
            .signed_lock_txids(fx.wallet, &pool::shield_step(0)))
        .contains(&lock.txid)
    );
    assert_eq!(
        *lib.drain_claimed.lock().unwrap(),
        [true],
        "under the claim"
    );
    assert_eq!(
        *lib.drain_floors.lock().unwrap(),
        [drain::drain_floor(pool_fee())]
    );
    drop(fx.s.coinjoin.claim_sweep(fx.wallet).unwrap());
}

fn core_max_quote(inputs: u32) -> crate::platform::credits::CoreMax {
    crate::platform::credits::CoreMax {
        inputs,
        left_duffs: 7_000_000,
        arriving_duffs: 1_500_000,
    }
}

/// A build that is not the plan (a coin that came after it, a planned
/// coin gone, a new coin to a planned address in place of a spent one) is
/// refused at the signer before any signature: nothing is funded, the step
/// holds no lock and reads not sent, and the next Max funds on the same
/// step.
#[test]
fn core_max_signs_exactly_the_planned_coins() {
    use super::super::flows::DispatchState;
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let total = sum(&account().coins);
    let step = pool::shield_step(0);
    let mut swapped = account().coins;
    swapped[2].outpoint.vout = 9;
    swapped[2].value -= 1;
    let builds = [
        [account().coins, vec![bip44(9, 1_000)]].concat(),
        account().coins[..2].to_vec(),
        swapped,
    ];
    for built in builds {
        *lib.drain_built.lock().unwrap() = Some(built);
        assert_eq!(
            code(max_of(&fx, &lib, shield_grant(&fx, total))),
            "shielded.coins_changed"
        );
        assert!(lib.funded.lock().unwrap().is_empty());
        assert!(fx.s.leases.signed_lock_txids(fx.wallet, &step).is_empty());
        assert_ne!(
            fx.s.leases.funding_status(fx.wallet, &step),
            Some(DispatchState::MaybeSent)
        );
    }
    *lib.drain_built.lock().unwrap() = None;
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    max_of(&fx, &lib, shield_grant(&fx, total)).unwrap();
    assert!(!fx.s.leases.signed_lock_txids(fx.wallet, &step).is_empty());
}

/// A held coin of the account (a user's lock, a reservation: the library's
/// drain would take it) refuses Max before anything is signed; the grant
/// stays unused.
#[test]
fn core_max_refuses_a_held_coin_before_signing() {
    let fx = Fixture::new(0);
    let mut read = account();
    read.coins[1].held = true;
    let lib = maxer(&fx, read);
    assert_eq!(code(quote_of(&fx, &lib)), "shielded.coins_locked");
    let grant = shield_grant(&fx, sum(&account().coins));
    assert_eq!(
        code(max_of(&fx, &lib, grant.clone())),
        "shielded.coins_locked"
    );
    assert!(lib.funded.lock().unwrap().is_empty());
    assert!(
        lib.drain_floors.lock().unwrap().is_empty(),
        "no funding call"
    );
    unused(&fx, &grant);
}

/// More than 50 coins refuse Max before signing; 50 coins fund.
#[test]
fn core_max_refuses_past_50_inputs_before_signing() {
    let fx = Fixture::new(0);
    let coins = |n: u32| CoreCoins {
        coins: (0..n).map(|i| bip44(i, 1_000_000)).collect(),
        ..CoreCoins::default()
    };
    let lib = maxer(&fx, coins(51));
    let refused = ShieldedError::TooManyInputs {
        inputs: 51,
        max: 50,
    };
    assert_eq!(quote_of(&fx, &lib).unwrap_err(), refused);
    let grant = shield_grant(&fx, 51_000_000);
    assert_eq!(max_of(&fx, &lib, grant.clone()).unwrap_err(), refused);
    assert!(
        lib.drain_floors.lock().unwrap().is_empty(),
        "no funding call"
    );
    unused(&fx, &grant);

    let lib = maxer(&fx, coins(50));
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    max_of(&fx, &lib, shield_grant(&fx, 50_000_000)).unwrap();
    assert_eq!(lib.drained.lock().unwrap()[0].len(), 50);
}

/// Max is refused while the wallet mixes or another move of its coins
/// runs, before it plans: nothing funded, the grant unused.
#[test]
fn core_max_refused_while_mixing() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let grant = shield_grant(&fx, sum(&account().coins));
    fx.s.coinjoin.set_mixing_for_test(fx.wallet, true);
    assert_eq!(code(quote_of(&fx, &lib)), "shielded.mixing_active");
    assert_eq!(
        code(max_of(&fx, &lib, grant.clone())),
        "shielded.mixing_active"
    );
    fx.s.coinjoin.set_mixing_for_test(fx.wallet, false);
    let sweep = fx.s.coinjoin.claim_sweep(fx.wallet).unwrap();
    assert_eq!(code(quote_of(&fx, &lib)), "shielded.move_running");
    assert_eq!(
        code(max_of(&fx, &lib, grant.clone())),
        "shielded.move_running"
    );
    drop(sweep);
    assert!(lib.funded.lock().unwrap().is_empty());
    unused(&fx, &grant);
}

/// A grant below the coins' sum is refused before it is redeemed.
#[test]
fn drain_grant_caps_the_debit() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let total = sum(&account().coins);
    let short = shield_grant(&fx, total - 1);
    assert_eq!(
        max_of(&fx, &lib, short.clone()).unwrap_err(),
        PlatformError::GrantExceeded {
            purpose: super::super::flows::BudgetPurpose::Funding,
            needed: total,
            remaining: total - 1,
        }
        .into()
    );
    assert!(lib.funded.lock().unwrap().is_empty());
    unused(&fx, &short);
}

/// Coins that changed since the quote to a smaller sum (one spent, one
/// swapped for a smaller coin) are `coins_changed` under the quote's grant,
/// before the lease: nothing funded, the grant unused; a quote of the coins
/// as they are funds.
#[test]
fn core_max_refuses_coins_changed_since_the_quote() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    let quoted = quote_of(&fx, &lib).unwrap().grant.max_duffs;
    let spent = CoreCoins {
        coins: account().coins[..2].to_vec(),
        ..account()
    };
    let mut swapped = account();
    swapped.coins[2] = bip44(9, swapped.coins[2].value - 1);
    for read in [spent, swapped] {
        *lib.core.lock().unwrap() = read;
        let grant = shield_grant(&fx, quoted);
        assert_eq!(
            max_of(&fx, &lib, grant.clone()).unwrap_err(),
            ShieldedError::CoinsChanged
        );
        assert!(lib.funded.lock().unwrap().is_empty());
        unused(&fx, &grant);
    }
    let requoted = quote_of(&fx, &lib).unwrap().grant.max_duffs;
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    max_of(&fx, &lib, shield_grant(&fx, requoted)).unwrap();
    assert_eq!(lib.funded.lock().unwrap().len(), 1);
    drop(fx.s.coinjoin.claim_sweep(fx.wallet).unwrap());
}

/// A lock below the pool's floor, or none, is refused before signing,
/// with what Max would move (the figure held against the floor), what the
/// other accounts hold and what is still arriving.
#[test]
fn core_max_refuses_below_the_floor() {
    let fx = Fixture::new(0);
    let floor = drain::drain_floor(pool_fee());
    let fee = crate::fees::asset_lock_fee_bound(1);
    let one = |v: u64, left_duffs: u64| CoreCoins {
        coins: vec![bip44(0, v)],
        left_duffs,
        arriving_duffs: 0,
    };
    let below = |duffs: u64, left_duffs: u64, arriving_duffs: u64| ShieldedError::BelowMinimum {
        min: floor,
        max: Some(CoreMaxShort {
            duffs,
            left_duffs,
            arriving_duffs,
        }),
    };
    assert_eq!(
        quote_of(&fx, &maxer(&fx, one(floor + fee - 1, 0))).unwrap_err(),
        below(floor - 1, 0, 0)
    );
    assert_eq!(
        quote_of(&fx, &maxer(&fx, one(floor + fee - 1, 7_000))).unwrap_err(),
        below(floor - 1, 7_000, 0)
    );
    assert_eq!(
        quote_of(&fx, &maxer(&fx, CoreCoins::default())).unwrap_err(),
        below(0, 0, 0)
    );
    let arriving = CoreCoins {
        arriving_duffs: 90_000,
        left_duffs: 7_000,
        ..CoreCoins::default()
    };
    assert_eq!(
        quote_of(&fx, &maxer(&fx, arriving)).unwrap_err(),
        below(0, 7_000, 90_000)
    );
    quote_of(&fx, &maxer(&fx, one(floor + fee, 7_000))).unwrap();
}

/// A Max whose lock was signed and is out but not applied (the process
/// stopped, or the apply was refused) pends as a shield's: a new Max is
/// refused on it, and the resume applies that same outpoint, never a new
/// lock.
#[test]
fn kill_after_drain_lock_signed_resumes_same_outpoint() {
    let fx = Fixture::new(0);
    let lib = maxer(&fx, account());
    *lib.applies.lock().unwrap() = Some(Err(PlatformError::Unavailable));
    let total = sum(&account().coins);
    let outcome = max_of(&fx, &lib, shield_grant(&fx, total)).unwrap();
    assert_eq!(
        outcome.state,
        ShieldState::NotApplied {
            error: "platform.unavailable".into()
        }
    );
    let lock = lib.funded.lock().unwrap()[0].outpoint;
    let open = run(&fx, &lib, |_, lib| async move {
        pool::open_shields(&*lib).await
    })
    .unwrap();
    assert_eq!(
        open.iter().map(|o| o.lock.outpoint).collect::<Vec<_>>(),
        [lock]
    );
    assert_eq!(
        max_of(&fx, &lib, shield_grant(&fx, total)).unwrap_err(),
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

/// The read of the wallet's coins: BIP44 account 0's final coins are the
/// plan's (an InstantSend lock is final), held when locked, reserved, a
/// denomination Dash Core counts as mixed or off its address paths; one
/// not final yet (or an immature coinbase) is arriving; every other
/// account's spendable coin is left, a mixed denomination too.
#[test]
fn core_max_leaves_other_accounts() {
    use crate::coins::{CoinSnapshot, WalletCoin};
    use dashcore::{Address, TxOut};
    let coin = |n: u8, send_account: bool| {
        let address = Address::from_str("yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n")
            .unwrap()
            .assume_checked();
        let txout = TxOut {
            value: 1_000 * u64::from(n),
            script_pubkey: address.script_pubkey(),
        };
        let outpoint = OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let mut utxo = key_wallet::Utxo::new(outpoint, txout, address, 100, false);
        utxo.is_confirmed = true;
        WalletCoin {
            utxo,
            send_account,
            coinjoin_account: !send_account,
            counted_denomination: false,
            is_change: false,
            block_time: None,
            chain_locked: false,
            foreign_incoming: true,
            user_locked: false,
            dust_locked: false,
            reserved: false,
            lock_input: false,
            unresolved_lock: false,
            awaiting_history: false,
        }
    };
    let mut pending = coin(2, true);
    pending.utxo.is_confirmed = false;
    let mut ix = coin(3, true);
    ix.utxo.is_confirmed = false;
    ix.utxo.is_instantlocked = true;
    let mut user = coin(4, true);
    user.user_locked = true;
    let mut reserved = coin(5, true);
    reserved.reserved = true;
    let mut coinbase = coin(7, true);
    coinbase.utxo.is_coinbase = true;
    coinbase.utxo.height = 150;
    let mut denomination = coin(8, true);
    denomination.counted_denomination = true;
    let mut denomination_other = coin(14, false);
    denomination_other.counted_denomination = true;
    let mut locked_other = coin(12, true);
    locked_other.user_locked = true;
    let snapshot = CoinSnapshot {
        height: 200,
        coins: vec![
            coin(1, true),
            pending,
            ix,
            user,
            reserved,
            coin(6, true),
            coinbase,
            denomination,
            // Other accounts: BIP32, DashPay receiving, CoinJoin, a locked one,
            // a CoinJoin denomination.
            coin(10, true),
            coin(11, true),
            coin(13, false),
            locked_other,
            denomination_other,
        ],
    };
    let path = |p: &str| Some(DerivationPath::from_str(p).unwrap());
    let account: std::collections::HashMap<OutPoint, Option<DerivationPath>> = [
        (1, path("m/44'/1'/0'/0/1")),
        (2, path("m/44'/1'/0'/0/2")),
        (3, path("m/44'/1'/0'/1/0")),
        (4, path("m/44'/1'/0'/0/4")),
        (5, path("m/44'/1'/0'/0/5")),
        (6, None),
        (7, path("m/44'/1'/0'/0/7")),
        (8, path("m/44'/1'/0'/0/8")),
    ]
    .into_iter()
    .map(|(n, p)| (OutPoint::new(Txid::from_byte_array([n; 32]), 0), p))
    .collect();
    let read = core_max::coins_of(snapshot, &account, dashcore::Network::Testnet);
    let got: Vec<(u8, bool)> = (read.coins.iter())
        .map(|c| (c.outpoint.txid.to_byte_array()[0], c.held))
        .collect();
    assert_eq!(
        got,
        [
            (1, false),
            (3, false),
            (4, true),
            (5, true),
            (6, true),
            (8, true)
        ]
    );
    assert_eq!(read.arriving_duffs, 2_000 + 7_000);
    assert_eq!(read.left_duffs, 10_000 + 11_000 + 13_000 + 14_000);
}
