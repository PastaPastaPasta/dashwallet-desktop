use dpp::state_transition::StateTransitionEstimatedFeeValidation;
use dpp::state_transition::identity_create_from_addresses_transition::IdentityCreateFromAddressesTransition;
use dpp::state_transition::identity_create_from_addresses_transition::v0::IdentityCreateFromAddressesTransitionV0;
use dpp::state_transition::public_key_in_creation::IdentityPublicKeyInCreation;

use super::*;

const KEYS: usize = 3;

fn version() -> &'static PlatformVersion {
    PlatformVersion::latest()
}

fn address(byte: u8) -> PlatformAddress {
    PlatformAddress::P2pkh([byte; 20])
}

fn balances(entries: &[(u8, u64)]) -> BTreeMap<PlatformAddress, u64> {
    entries.iter().map(|(b, c)| (address(*b), *c)).collect()
}

fn nonces(entries: &[(u8, u64)]) -> BTreeMap<PlatformAddress, u32> {
    entries
        .iter()
        .map(|(b, _)| (address(*b), *b as u32))
        .collect()
}

fn plan(entries: &[(u8, u64)], funding: u64) -> Result<RegisterPlan, PlatformError> {
    register_plan(
        (&balances(entries), &nonces(entries)),
        funding,
        KEYS,
        version(),
    )
}

fn reserve(inputs: usize) -> u64 {
    REGISTER_FEE_BAND * register_fee(version(), inputs, KEYS)
}

/// The fee is dpp's own minimum for an identity create from addresses with
/// these inputs and keys and no output.
#[test]
fn the_fee_is_dpps_minimum_for_the_transition() {
    for inputs in 1..=3u8 {
        let transition =
            IdentityCreateFromAddressesTransition::V0(IdentityCreateFromAddressesTransitionV0 {
                public_keys: vec![IdentityPublicKeyInCreation::V0(Default::default()); KEYS],
                inputs: (1..=inputs).map(|b| (address(b), (1, 1_000_000))).collect(),
                output: None,
                fee_strategy: Vec::new(),
                user_fee_increase: 0,
                input_witnesses: Vec::new(),
            });
        assert_eq!(
            register_fee(version(), inputs as usize, KEYS),
            transition.calculate_min_required_fee(version()).unwrap(),
            "{inputs} inputs"
        );
    }
}

/// Input 0 is the first address above the reserve in address order; it
/// keeps three fees unclaimed; the claims are exactly the funding, with no
/// change; each input signs at its stored nonce + 1.
#[test]
fn input_0_keeps_three_fees_and_the_claims_are_the_funding() {
    let funding = 25_000_000;
    let one = reserve(1);
    let p = plan(
        &[(1, 1_000), (2, funding + one + 7), (3, 900_000_000)],
        funding,
    )
    .unwrap();
    assert_eq!(p.inputs, [(address(2), (3, funding))].into());
    assert_eq!(p.funding_credits, funding);
    assert_eq!(p.fee_reserve_credits, one);
    assert_eq!(p.fee_credits, register_fee(version(), 1, KEYS));
    assert_eq!(p.debit_credits(), funding + one);
    // Input 0 short of its claim and the reserve: a second input claims the
    // rest, and the reserve is three fees for two inputs.
    let two = reserve(2);
    let p = plan(&[(2, two + 10_000_000), (3, 900_000_000)], funding).unwrap();
    assert_eq!(
        p.inputs,
        [
            (address(2), (3, 10_000_000)),
            (address(3), (4, funding - 10_000_000))
        ]
        .into()
    );
    assert_eq!(p.fee_reserve_credits, two);
    assert_eq!(p.funding_credits, funding);
}

/// Less than dpp's funding minimum is planned at the minimum.
#[test]
fn the_funding_is_at_least_dpps_minimum() {
    let p = plan(&[(2, 900_000_000)], 1).unwrap();
    assert_eq!(p.funding_credits, min_funding(version()));
}

/// No address above the reserve, or not enough in all: refused before
/// anything is written.
#[test]
fn a_plan_that_cannot_pay_is_refused() {
    let funding = 25_000_000;
    let r = reserve(1);
    for entries in [vec![(1, r)], vec![(1, r + 1), (2, 1_000)], vec![]] {
        let e = plan(&entries, funding).unwrap_err();
        assert_eq!(e.code(), "platform.insufficient_credits", "{entries:?}");
    }
    // An address Platform holds no nonce for is not planned blind.
    let entries = [(2, 900_000_000)];
    let e = register_plan(
        (&balances(&entries), &BTreeMap::new()),
        funding,
        KEYS,
        version(),
    )
    .unwrap_err();
    assert_eq!(e, PlatformError::Unavailable);
}

/// A reserve for two inputs leaves the first address out (it holds less);
/// the next is input 0, and the last input's claim below the input minimum
/// is raised to it, so the identity receives the claims' sum.
#[test]
fn a_last_claim_raised_to_the_input_minimum_is_funded() {
    let funding = 25_000_000;
    let min_input = version()
        .dpp
        .state_transitions
        .address_funds
        .min_input_amount;
    let short = 50_000;
    let p = plan(
        &[
            (1, reserve(1) + 200_000),
            (2, reserve(2) + funding - short),
            (3, 900_000_000),
        ],
        funding,
    )
    .unwrap();
    assert_eq!(
        p.inputs,
        [
            (address(2), (3, funding - short)),
            (address(3), (4, min_input))
        ]
        .into()
    );
    assert_eq!(p.funding_credits, funding - short + min_input);
    assert_eq!(p.fee_reserve_credits, reserve(2));
    assert_eq!(p.fee_credits, register_fee(version(), 2, KEYS));
}

/// The flag: off on mainnet, on for every test network.
#[test]
fn the_flag_is_off_on_mainnet_only() {
    assert_eq!(
        refuse_on_mainnet(&DashNetwork::Mainnet),
        Err(PlatformError::FeatureOff {
            feature: "platform_registration".into()
        })
    );
    for on in [
        DashNetwork::Testnet,
        DashNetwork::Regtest,
        DashNetwork::Devnet {
            name: "devnet-a".into(),
        },
    ] {
        assert_eq!(refuse_on_mainnet(&on), Ok(()), "{on:?}");
    }
}

/// The reserve is three fees for exactly the inputs taken: an address above
/// the one-input reserve but below the two-input one is left out rather
/// than kept at a two-input reserve.
#[test]
fn the_reserve_is_three_fees_for_the_inputs_taken() {
    let funding = 400_000_000;
    let p = plan(&[(1, reserve(1) + 500_000), (2, 600_000_000)], funding).unwrap();
    assert_eq!(p.inputs, [(address(2), (3, funding))].into());
    assert_eq!(p.fee_reserve_credits, reserve(1));
    assert_eq!(p.fee_reserve_credits, REGISTER_FEE_BAND * p.fee_credits);
}

/// Input 0 above the reserve by less than the input minimum: the refusal
/// names input 0's balance against the reserve and that minimum.
#[test]
fn a_claim_below_the_input_minimum_reports_input_0() {
    let min_input = version()
        .dpp
        .state_transitions
        .address_funds
        .min_input_amount;
    let e = plan(&[(1, reserve(1) + 1), (2, 1_000_000_000)], 25_000_000).unwrap_err();
    assert_eq!(
        e,
        PlatformError::InsufficientCredits {
            needed: reserve(1) + min_input,
            available: reserve(1) + 1,
        }
    );
}
