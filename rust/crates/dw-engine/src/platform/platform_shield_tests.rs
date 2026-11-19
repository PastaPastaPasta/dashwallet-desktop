//! The quote of a move from the Platform addresses into Shielded
//! (C-routes C1a-1).

use super::*;
use crate::platform::{ShieldedError, ShieldedSyncWait};

impl World {
    fn shield_quote(&self, amount: WithdrawAmount) -> Result<PlatformShieldQuote, CreditsError> {
        self.call(move |net, s, w, id| async move {
            shield_quote(&*net, &s, &w, id, (PlatformVersion::latest(), amount)).await
        })
    }

    fn bech32m(&self, n: usize) -> String {
        bech32m(&self.address(n), self.network())
    }
}

fn reserve() -> u64 {
    platform_wallet::shield_fee_reserve_credits(PlatformVersion::latest()).unwrap()
}

fn pool_fee() -> u64 {
    shielded_pool::platform_shield_fee(PlatformVersion::latest()).unwrap()
}

/// The quote plans on the proved balances: input 0 is the first address in
/// `PlatformAddress` order above the reserve and keeps it, its claim carries
/// the pool fee, and no claim exceeds what its address holds. Max is the
/// usable balance less the reserve.
#[test]
fn a_shield_quote_never_exceeds_the_proved_balances() {
    let w = World::new();
    let mut by_order: Vec<usize> = (0..3).collect();
    by_order.sort_by_key(|n| w.address(*n));
    let (first, second, third) = (by_order[0], by_order[1], by_order[2]);
    // The first in order holds only the reserve: it is left out.
    w.net.fund(w.address(first), reserve());
    w.net.fund(w.address(second), 2 * DASH);
    w.net.fund(w.address(third), 5 * DASH);
    let (fee, reserve) = (pool_fee(), reserve());
    assert!(fee > 0 && reserve >= fee);

    let max = w.shield_quote(WithdrawAmount::All).unwrap();
    assert_eq!(max.credits, 7 * DASH - reserve);
    assert_eq!(
        max.inputs,
        [
            PlatformAddressRow {
                address: w.bech32m(second),
                credits: 2 * DASH - reserve + fee,
            },
            PlatformAddressRow {
                address: w.bech32m(third),
                credits: 5 * DASH,
            },
        ]
    );
    let some = w
        .shield_quote(WithdrawAmount::Credits { credits: DASH })
        .unwrap();
    assert_eq!(
        some.inputs,
        [PlatformAddressRow {
            address: w.bech32m(second),
            credits: DASH + fee,
        }]
    );
    for q in [&max, &some] {
        assert_eq!((q.pool_fee_credits, q.fee_reserve_credits), (fee, reserve));
        assert_eq!(q.debit_credits, q.credits + fee + reserve);
        assert_eq!(
            (q.grant.max_duffs, q.grant.max_credits),
            (0, q.debit_credits)
        );
        for row in &q.inputs {
            let n = (0..3).find(|n| w.bech32m(*n) == row.address).unwrap();
            assert!(row.credits <= w.net.credits(w.address(n)));
        }
        assert!(w.net.credits(w.address(second)) - q.inputs[0].credits >= reserve - fee);
    }

    match w.shield_quote(WithdrawAmount::Credits {
        credits: max.credits + 1,
    }) {
        Err(CreditsError::Platform(PlatformError::InsufficientCredits { needed, available })) => {
            assert_eq!((needed, available), (max.credits + 1 + reserve, 7 * DASH));
        }
        other => panic!("{other:?}"),
    }
    // The pool fee clears the input minimum, so one credit shields.
    let one = (w.shield_quote(WithdrawAmount::Credits { credits: 1 })).unwrap();
    assert_eq!(one.inputs[0].credits, 1 + fee);
    assert_eq!(
        w.shield_quote(WithdrawAmount::Credits { credits: 0 })
            .unwrap_err()
            .code(),
        "credits.send_out_of_range"
    );
}

/// With nothing above the reserve there is nothing to shield, and the
/// shortfall is against the largest address (input 0 is one address), so
/// it always reads as short.
#[test]
fn a_shield_quote_without_an_address_above_the_reserve_is_refused() {
    let w = World::new();
    w.net.fund(w.address(0), reserve());
    w.net.fund(w.address(1), reserve() / 2);
    match w.shield_quote(WithdrawAmount::All) {
        Err(CreditsError::Platform(PlatformError::InsufficientCredits { needed, available })) => {
            assert_eq!((needed, available), (1 + reserve(), reserve()));
        }
        other => panic!("{other:?}"),
    }
}

/// A pending move (its outcome unknown) refuses the shield.
#[test]
fn the_shield_quote_refuses_beside_a_pending_move() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::Lost);
    let one = WithdrawAmount::Credits { credits: DASH };
    let pending = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(pending.state, PlatformActivityState::Pending);
    match w.shield_quote(WithdrawAmount::All) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, pending.id),
        other => panic!("{other:?}"),
    }
}

/// A send failed by height may still execute, so it refuses the shield;
/// once its nonces are taken the quote plans again.
#[test]
fn the_shield_quote_refuses_beside_a_hold() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let held = w.held_send(WithdrawAmount::Credits { credits: DASH }, &to);
    match w.shield_quote(WithdrawAmount::All) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, held.id),
        other => panic!("{other:?}"),
    }
    w.lands(0);
    w.shield_quote(WithdrawAmount::All).unwrap();
}

/// Under an enforcing mode with the latch set, an unverified read quotes
/// nothing.
#[test]
fn an_unverified_read_quotes_no_shield() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    gate_session(
        &w.f.s,
        TrustMode::Enforcing,
        FakePlatform::new(Proof::Verified),
    );
    w.net.unverified.store(true, Ordering::SeqCst);
    assert_eq!(
        w.shield_quote(WithdrawAmount::All).unwrap_err().code(),
        "platform.context_unavailable"
    );
    w.net.unverified.store(false, Ordering::SeqCst);
    w.shield_quote(WithdrawAmount::All).unwrap();
}

/// The pool's refusals reach the caller with the pool's codes, and its
/// `Platform` variant as the credits' own.
#[test]
fn the_pools_refusals_keep_their_codes() {
    let e = CreditsError::from(ShieldedError::not_synced(ShieldedSyncWait::Account));
    assert_eq!(e.code(), "shielded.not_synced");
    assert!(matches!(e, CreditsError::Shielded(_)));
    let e = CreditsError::from(ShieldedError::Platform(PlatformError::Timeout));
    assert_eq!(e, CreditsError::Platform(PlatformError::Timeout));
    assert_eq!(e.platform(), Some(&PlatformError::Timeout));
}
