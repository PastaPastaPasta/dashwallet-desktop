//! C-funding C4a-1: a Platform-funded registration's quote, the start's
//! checks and its stored kind (F2–F5, F9).

use dpp::address_funds::PlatformAddress;

use super::*;
use crate::platform::registration::{PlatformRegistrationQuote, RegistrationQuote};
use crate::platform::registration_flow::is_unsupported;
use crate::platform::registration_platform::{
    FEATURE, REGISTER_FEE_BAND, min_funding, register_fee,
};
use crate::platform::registration_rows::PlatformPlan;

const HELD: u64 = 900_000_000;

fn platform(funding_credits: u64, fee_reserve_credits: u64) -> RegistrationRequest {
    RegistrationRequest {
        funding: RegistrationFunding::PlatformAddresses {
            funding_credits,
            fee_reserve_credits,
        },
        ..request(PLAIN)
    }
}

fn quoted(q: &PlatformRegistrationQuote) -> RegistrationRequest {
    platform(q.funding_credits, q.fee_reserve_credits)
}

fn funded(lib: &Fake) {
    lib.w().platform = [(PlatformAddress::P2pkh([2; 20]), HELD)].into();
}

fn quote(f: &Fixture, lib: &Arc<Fake>) -> Result<RegistrationQuote, RegistrationError> {
    f.engine
        .block_on(registration::quote(&**lib, &platform(0, 0)))
}

fn start(f: &Fixture, lib: &Arc<Fake>, req: RegistrationRequest) -> RegistrationError {
    f.engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(lib),
            req,
            "unused".into(),
        ))
        .unwrap_err()
}

/// F5, F9: the identity receives what its name takes; the first address
/// keeps three fees; nothing comes from the Core balance; the grant covers
/// what the addresses may lose and the name.
#[test]
fn the_quote_names_what_leaves_the_platform_balance_and_the_grant_covers_it() {
    let f = fixture(false);
    let lib = f.fake();
    funded(&lib);
    let q = quote(&f, &lib).unwrap();
    let version = PlatformVersion::get(PROTOCOL_VERSION_13).unwrap();
    let names = f.engine.block_on(lib.name_credits(PLAIN)).unwrap();
    let keys = crate::platform::keys_policy::registration_specs().len();
    let fee = register_fee(version, 1, keys);
    let funding = names.max(min_funding(version));
    let p = q.platform.clone().unwrap();
    assert_eq!(
        p,
        PlatformRegistrationQuote {
            funding_credits: funding,
            fee_credits: fee,
            fee_reserve_credits: REGISTER_FEE_BAND * fee,
            debit_credits: funding + REGISTER_FEE_BAND * fee,
        }
    );
    assert_eq!(
        (q.lock_duffs, q.fee_duffs, q.total_duffs, q.available_duffs),
        (0, 0, 0, 0)
    );
    assert_eq!(q.grant.max_duffs, 0);
    assert_eq!(q.grant.max_credits, p.debit_credits + names);
    assert_eq!(q.remaining_credits, funding - names);
    assert!(q.readiness.is_none());
    // Only Platform funding carries it.
    let core = f
        .engine
        .block_on(registration::quote(&*lib, &request(PLAIN)));
    assert!(core.unwrap().platform.is_none());
}

/// F9: a move that may still execute refuses the plan; so does a balance
/// that cannot pay the funding and the reserve.
#[test]
fn the_quote_refuses_beside_a_held_move_and_without_enough() {
    let f = fixture(false);
    let lib = f.fake();
    let e = quote(&f, &lib).unwrap_err();
    assert_eq!(e.code(), "platform.insufficient_credits", "{e:?}");
    funded(&lib);
    lib.w().platform_held = Some("move-7-0".into());
    let e = quote(&f, &lib).unwrap_err();
    assert_eq!(e.code(), "credits.move_pending", "{e:?}");
    let e = start(&f, &lib, platform(0, 0));
    assert_eq!(e.code(), "credits.move_pending", "{e:?}");
    assert!(f.rows().is_empty());
}

/// F4: a start passes back the numbers its quote showed, and is refused
/// with nothing written until the flow step exists (C4a-2); other numbers
/// are `registration.quote_changed`.
#[test]
fn a_start_checks_the_quoted_numbers_and_writes_nothing() {
    let f = fixture(false);
    let lib = f.fake();
    funded(&lib);
    let p = quote(&f, &lib).unwrap().platform.unwrap();
    let e = start(&f, &lib, quoted(&p));
    assert!(
        matches!(&e, RegistrationError::Platform(PlatformError::FeatureOff { feature })
            if feature == FEATURE),
        "{e:?}"
    );
    for changed in [
        platform(p.funding_credits + 1, p.fee_reserve_credits),
        platform(p.funding_credits, p.fee_reserve_credits - 1),
        platform(0, 0),
    ] {
        let e = start(&f, &lib, changed);
        assert_eq!(e, RegistrationError::QuoteChanged);
    }
    // Another Platform balance since the quote plans two inputs, and so
    // another reserve.
    let version = PlatformVersion::get(PROTOCOL_VERSION_13).unwrap();
    let keys = crate::platform::keys_policy::registration_specs().len();
    let two = REGISTER_FEE_BAND * register_fee(version, 2, keys);
    lib.w().platform = [
        (PlatformAddress::P2pkh([2; 20]), two + 1_000_000),
        (PlatformAddress::P2pkh([3; 20]), HELD),
    ]
    .into();
    assert_ne!(two, p.fee_reserve_credits);
    assert_eq!(start(&f, &lib, quoted(&p)), RegistrationError::QuoteChanged);
    assert!(f.rows().is_empty());
}

/// F2: a Platform row, planned or not, is never funded, discarded or
/// advanced by this build.
#[test]
fn a_platform_row_is_never_run_by_this_build() {
    let f = fixture(false);
    for plan in [
        None,
        Some(PlatformPlan {
            inputs: vec![("tdash1qa".into(), 3, 25_000_000)],
            identity: "Carol1dentity".into(),
        }),
    ] {
        let mut row = core_row(PLAIN);
        row.funding = Funding::PlatformAddresses {
            funding_credits: 25_000_000,
            fee_reserve_credits: 64_500_000,
            plan,
        }
        .encode();
        let draft = f.insert(row);
        assert!(is_unsupported(&f.row(draft)));
    }
}

/// Mainnet: off (flag `platform_registration`). On mainnet the quote and
/// the start are refused before any name is looked up (DAPI is down) or the
/// addresses are read (the held move is never reached), the grant redeemed
/// or a row written; a Core-funded registration is unchanged there.
#[test]
fn mainnet_refuses_a_platform_registration_before_anything_is_written() {
    let f = fixture(false);
    let lib = f.fake();
    funded(&lib);
    let p = quote(&f, &lib).unwrap().platform.unwrap();
    lib.w().mainnet = true;
    lib.w().platform_held = Some("move-7-0".into());
    lib.w().names_offline = true;
    let off = |e: &RegistrationError| {
        matches!(e, RegistrationError::Platform(PlatformError::FeatureOff { feature })
            if feature == FEATURE)
    };
    let e = quote(&f, &lib).unwrap_err();
    assert!(off(&e), "{e:?}");
    let e = start(&f, &lib, quoted(&p));
    assert!(off(&e), "{e:?}");
    assert!(f.rows().is_empty());
    lib.w().names_offline = false;
    let core = f
        .engine
        .block_on(registration::quote(&*lib, &request(PLAIN)));
    assert!(core.unwrap().platform.is_none());
}
