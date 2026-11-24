use super::*;

const RESERVE: u64 = 1_000;

fn address(byte: u8) -> PlatformAddress {
    PlatformAddress::P2pkh([byte; 20])
}

fn balances(entries: &[(u8, u64)]) -> BTreeMap<PlatformAddress, u64> {
    entries.iter().map(|(b, c)| (address(*b), *c)).collect()
}

fn rules() -> Input0Rules {
    Input0Rules {
        reserve: RESERVE,
        min_input: 100,
        max_inputs: 3,
        min_claim_0: 1,
        min_total: 1,
        exact: false,
    }
}

fn credits(credits: u64) -> WithdrawAmount {
    WithdrawAmount::Credits { credits }
}

fn claims(plan: &Input0Plan) -> Vec<(u8, u64)> {
    (plan.inputs.iter())
        .map(|(a, c)| match a {
            PlatformAddress::P2pkh(h) => (h[0], *c),
            PlatformAddress::P2sh(h) => (h[0], *c),
        })
        .collect()
}

/// Input 0 is the first address in `PlatformAddress` order, not the
/// largest, and keeps the reserve unclaimed, for an amount and for Max.
#[test]
fn input_0_keeps_the_reserve() {
    let b = balances(&[(1, 5_000), (2, 90_000), (3, 40_000)]);
    let some = plan(&b, credits(4_500), &rules()).unwrap();
    assert_eq!(claims(&some), [(1, 4_000), (2, 500)]);
    let one = plan(&b, credits(3_000), &rules()).unwrap();
    assert_eq!(claims(&one), [(1, 3_000)]);
    let max = plan(&b, WithdrawAmount::All, &rules()).unwrap();
    assert_eq!(claims(&max), [(1, 4_000), (2, 90_000), (3, 40_000)]);
    for p in [some, one, max] {
        assert_eq!(p.input_0().0, address(1));
        assert!(b[&address(1)] - p.input_0().1 >= RESERVE);
    }
}

/// An address at or below the reserve cannot pay the fee: the plan starts
/// at the first one above it, and leaves the ones before out.
#[test]
fn a_dust_first_address_is_skipped() {
    let b = balances(&[(1, RESERVE), (2, 300), (3, 2_000), (4, 50)]);
    let p = plan(&b, WithdrawAmount::All, &rules()).unwrap();
    assert_eq!(claims(&p), [(3, 2_000 - RESERVE)]);
    // One credit above the reserve is enough for input 0.
    let b = balances(&[(1, RESERVE + 1), (3, 2_000)]);
    let p = plan(&b, credits(1), &rules()).unwrap();
    assert_eq!(claims(&p), [(1, 1)]);
}

/// Max is what the usable addresses hold less the reserve: later dust
/// below the input minimum and addresses past the input cap are not usable.
#[test]
fn max_is_the_usable_balance_less_the_reserve() {
    let b = balances(&[(1, 3_000), (2, 99), (3, 100), (4, 7_000), (5, 9_000)]);
    let p = plan(&b, WithdrawAmount::All, &rules()).unwrap();
    let usable = 3_000 + 100 + 7_000;
    assert_eq!(p.credits, usable - RESERVE);
    assert_eq!(claims(&p), [(1, 2_000), (3, 100), (4, 7_000)]);
    assert_eq!(
        plan(&b, credits(p.credits), &rules()).unwrap().inputs,
        p.inputs
    );
    assert_eq!(
        plan(&b, credits(p.credits + 1), &rules()),
        Err(Input0Refused::Short {
            needed: p.credits + 1 + RESERVE,
            available: usable,
        })
    );
}

/// Nothing above the reserve, or no amount: refused, the shortfall against
/// the largest address, since input 0 is one address.
#[test]
fn nothing_to_plan_is_refused() {
    let b = balances(&[(1, RESERVE), (2, 500)]);
    assert_eq!(
        plan(&b, WithdrawAmount::All, &rules()),
        Err(Input0Refused::Short {
            needed: 1 + RESERVE,
            available: RESERVE,
        })
    );
    assert_eq!(
        plan(&balances(&[(1, 5_000)]), credits(0), &rules()),
        Err(Input0Refused::TotalBelow { min: 1 })
    );
}

/// A later input's last share below the input minimum claims the minimum
/// (it holds at least that), so the claims may exceed the amount.
#[test]
fn a_small_last_share_claims_the_input_minimum() {
    let b = balances(&[(1, 1_500), (2, 800)]);
    let p = plan(&b, credits(530), &rules()).unwrap();
    assert_eq!(claims(&p), [(1, 500), (2, 100)]);
    assert_eq!(p.credits, 530);
}

/// The caller's bounds (a top-up's dpp minimums, C3a): input 0's claim and
/// the claims' total, each refused typed.
#[test]
fn the_minimum_claim_and_total_refuse() {
    let b = balances(&[(1, 1_500), (2, 5_000)]);
    let bounded = Input0Rules {
        min_claim_0: 600,
        min_total: 2_000,
        ..rules()
    };
    // Input 0 can give 500 at most.
    assert_eq!(
        plan(&b, credits(3_000), &bounded),
        Err(Input0Refused::ClaimBelow { min: 600 })
    );
    let b = balances(&[(1, 9_000), (2, 5_000)]);
    assert_eq!(
        plan(&b, credits(599), &bounded),
        Err(Input0Refused::ClaimBelow { min: 600 })
    );
    assert_eq!(
        plan(&b, credits(1_999), &bounded),
        Err(Input0Refused::TotalBelow { min: 2_000 })
    );
    assert_eq!(
        claims(&plan(&b, credits(2_000), &bounded).unwrap()),
        [(1, 2_000)]
    );
    assert_eq!(
        plan(&balances(&[(1, 2_500)]), WithdrawAmount::All, &bounded),
        Err(Input0Refused::TotalBelow { min: 2_000 })
    );
}

/// Over many address sets and amounts: a plan exactly when the amount is
/// within Max (computed here on its own), no claim exceeds its address's
/// proved balance, input 0 keeps the reserve and is the first address above
/// it, later inputs hold the input minimum, the claims cover the amount,
/// and the count stays within the cap.
#[test]
fn a_plan_never_claims_past_the_proved_balances() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    let r = rules();
    let mut planned = 0;
    for _ in 0..2_000 {
        let n = 1 + next(6) as u8;
        let b: BTreeMap<_, _> = (0..n)
            .map(|i| (address(i * 7 + 1), next(4) * next(3_000)))
            .collect();
        let amount = match next(3) {
            0 => WithdrawAmount::All,
            _ => credits(1 + next(12_000)),
        };
        let first = (b.iter()).find(|(_, c)| **c > RESERVE).map(|(a, _)| *a);
        let max = first.map_or(0, |first| {
            let later = (b.range(first..).skip(1))
                .filter(|(_, c)| **c >= r.min_input)
                .take(r.max_inputs - 1);
            (b[&first] + later.map(|(_, c)| *c).sum::<u64>()) - RESERVE
        });
        let fits = match amount {
            WithdrawAmount::All => max > 0,
            WithdrawAmount::Credits { credits } => credits <= max,
        };
        let planned_now = plan(&b, amount, &r);
        assert_eq!(
            planned_now.is_ok(),
            fits,
            "{b:?} {amount:?} {planned_now:?}"
        );
        let Ok(p) = planned_now else {
            continue;
        };
        planned += 1;
        assert_eq!(Some(p.input_0().0), first, "{b:?} {amount:?}");
        assert!(b[&p.input_0().0] - p.input_0().1 >= RESERVE);
        for (i, (a, c)) in p.inputs.iter().enumerate() {
            assert!(*c > 0 && *c <= b[a], "{b:?} {amount:?} {p:?}");
            assert!(i == 0 || *c >= r.min_input, "{b:?} {amount:?} {p:?}");
        }
        assert!(p.inputs.values().sum::<u64>() >= p.credits);
        assert!(p.inputs.len() <= r.max_inputs);
        let expected = match amount {
            WithdrawAmount::All => max,
            WithdrawAmount::Credits { credits } => credits,
        };
        assert_eq!(p.credits, expected);
    }
    assert!(planned > 500, "{planned}");
}

/// An exact route (the top-up, C3-3): the claims total the amount, so a
/// short last claim borrows from the input before, which keeps its own
/// minimum; a shield-style plan claims the minimum instead.
#[test]
fn an_exact_plan_borrows_the_short_last_claim() {
    let b = balances(&[(1, 1_500), (2, 5_000)]);
    let exact = Input0Rules {
        exact: true,
        min_claim_0: 100,
        ..rules()
    };
    // Input 0 gives 500; the 30 left would be below the minimum of 100.
    let p = plan(&b, credits(530), &exact).unwrap();
    assert_eq!(claims(&p), [(1, 430), (2, 100)]);
    assert_eq!(p.inputs.values().sum::<u64>(), 530);
    assert_eq!(
        claims(&plan(&b, credits(530), &rules()).unwrap()),
        [(1, 500), (2, 100)]
    );
    // Input 0 cannot lend below its own minimum.
    let tight = Input0Rules {
        min_claim_0: 450,
        ..exact
    };
    assert_eq!(
        plan(&b, credits(530), &tight),
        Err(Input0Refused::ClaimBelow { min: 100 })
    );
    // Max takes every later input whole: nothing to borrow.
    let p = plan(&b, WithdrawAmount::All, &exact).unwrap();
    assert_eq!(claims(&p), [(1, 500), (2, 5_000)]);
}
