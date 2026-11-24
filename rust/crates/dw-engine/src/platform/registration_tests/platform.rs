//! C-funding C4a: a Platform-funded registration's quote, the start's
//! checks, its stored kind and its funding step (F2–F10, F22).

use dpp::address_funds::PlatformAddress;

use super::*;
use crate::platform::errors::CreditsError;
use crate::platform::platform_moves::RegisterOutcome;
use crate::platform::registration::RegistrationWait;
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

/// F4: a start passes back the numbers its quote showed; other numbers
/// are `registration.quote_changed`, with nothing written.
#[test]
fn a_start_checks_the_quoted_numbers_and_writes_nothing() {
    let f = fixture(false);
    let lib = f.fake();
    funded(&lib);
    let p = quote(&f, &lib).unwrap().platform.unwrap();
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

/// F2: this build reads and runs a Platform row, planned or not.
#[test]
fn a_platform_row_is_run_by_this_build() {
    let f = fixture(false);
    for plan in [None, Some(platform_plan_of(25_000_000))] {
        let draft = f.insert(platform_row(plan));
        assert!(!is_unsupported(&f.row(draft)));
    }
}

const FUNDING: u64 = 25_000_000;
const RESERVE: u64 = 64_500_000;

fn platform_row(plan: Option<PlatformPlan>) -> NewRegistration {
    NewRegistration {
        identity_index: Some(0),
        funding: Funding::PlatformAddresses {
            funding_credits: FUNDING,
            fee_reserve_credits: RESERVE,
            plan,
        }
        .encode(),
        phase: Phase::KeysPrepared.as_str().into(),
        ..core_row(PLAIN)
    }
}

fn plan_of(row: &RegistrationRow) -> Option<PlatformPlan> {
    match Funding::decode(&row.funding) {
        crate::platform::registration_rows::Decoded::Known(Funding::PlatformAddresses {
            plan,
            ..
        }) => plan,
        other => panic!("{other:?}"),
    }
}

/// One scripted pass: whether it writes a plan, and how it ends.
fn script(
    lib: &Fake,
    passes: impl IntoIterator<Item = (bool, Result<RegisterOutcome, CreditsError>)>,
) {
    lib.w().platform_fund.extend(passes);
}

/// F9, F10: the step writes the plan into the row before anything is
/// signed; a proved identity is adopted and the flow goes on to its name;
/// the pass names the row, its index and the quoted numbers.
#[test]
fn a_proved_create_registers_the_planned_identity() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(None));
    let id = platform_identity();
    script(&lib, [(true, Ok(RegisterOutcome::Registered(id.clone())))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(row.identity.as_deref(), Some(id.as_str()));
    assert!(phase(&row) > Phase::IdentityRegistered, "{}", row.phase);
    assert_eq!(
        plan_of(&row),
        Some(platform_plan_of(FUNDING)),
        "the plan stays"
    );
    let asks = lib.w().platform_asks.clone();
    assert_eq!(asks.len(), 1);
    let ask = &asks[0];
    assert_eq!(
        (
            ask.draft,
            ask.index,
            ask.funding_credits,
            ask.fee_reserve_credits
        ),
        (draft, 0, FUNDING, RESERVE)
    );
    assert!(ask.plan.is_none() && ask.resign);
}

/// F6, F7: a create whose outcome is unknown keeps the row's plan and
/// waits `payment_unknown`; its funds are committed and it is not
/// discarded. The next pass passes the stored plan back, signs nothing new
/// in the same run (a double submit), and settles it by evidence.
#[test]
fn an_unknown_create_waits_keeps_its_plan_and_refuses_the_discard() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(None));
    script(&lib, [(true, Ok(RegisterOutcome::Unknown))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::KeysPrepared);
    assert_eq!(plan_of(&row), Some(platform_plan_of(FUNDING)));
    let status = f.status(&lib, draft);
    assert!(status.funds_committed);
    assert!(
        f.discard(&lib, draft).is_err(),
        "a signed plan is not discarded"
    );
    // The run's next pass: the stored plan, no second signature.
    script(
        &lib,
        [
            (true, Ok(RegisterOutcome::Unknown)),
            (true, Ok(RegisterOutcome::Registered(platform_identity()))),
        ],
    );
    let lease = f.lease();
    f.engine.block_on(async {
        let claim = (f.session.platform.registrations)
            .claim(&f.session, (f.wallet, draft))
            .await
            .unwrap()
            .unwrap();
        let driver = Driver::new(&f.session, &*lib, &claim, Some(lease), true);
        tokio::time::timeout(Duration::from_secs(120), driver.run())
            .await
            .unwrap()
            .unwrap();
    });
    let asks = lib.w().platform_asks.clone();
    assert_eq!(asks.len(), 3);
    assert!(
        asks[1..]
            .iter()
            .all(|a| a.plan == Some(platform_plan_of(FUNDING)))
    );
    assert_eq!(
        asks.iter().map(|a| a.resign).collect::<Vec<_>>(),
        [true, true, false],
        "one signature per run"
    );
    assert_eq!(f.row(draft).identity, Some(platform_identity()));
}

/// F22: a create failed by height parks `payment_held`, its plan kept.
#[test]
fn a_held_create_parks_for_the_way_out() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    script(&lib, [(false, Ok(RegisterOutcome::Held))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    // Parked, its status reads the stored record: until one fails by
    // height, the payment is unknown.
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::PaymentUnknown)
    );
    let record = format!(
        r#"{{"id":"register-{draft}-x","kind":"register","inputs":[],"credits":1,
            "fee_credits":1,"expected_duffs":0,"to":"x","at":1,"state":"failed",
            "can_land":true}}"#
    );
    let scope = dw_appdb::local_scope(&f.wallet.to_string());
    let key = format!("platform_address/move/register-{draft}-x");
    f.appdb()
        .set_setting_durable(&scope, &key, Some(&record))
        .unwrap();
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::PaymentHeld)
    );
    assert_eq!(plan_of(&f.row(draft)), Some(platform_plan_of(FUNDING)));
    assert!(f.status(&lib, draft).funds_committed);
}

/// F22: the exit executed: the plan is cleared and the row fails back to
/// Confirm (`registration.quote_changed`), free to discard.
#[test]
fn an_executed_exit_sends_the_row_back_to_confirm() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    script(&lib, [(false, Ok(RegisterOutcome::Exited))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(plan_of(&row), None);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("registration.quote_changed"), false)
    );
    assert!(!f.status(&lib, draft).funds_committed);
    f.discard(&lib, draft).unwrap();
}

/// F10, F12: nonces taken with no identity and no exit: an identity of the
/// seed at the row's index is adopted; else the row fails
/// `registration.refunded`, not retryable, its plan cleared.
#[test]
fn a_create_failed_in_a_block_is_refunded_unless_the_index_holds_one() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    script(&lib, [(false, Ok(RegisterOutcome::PaidFailure))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(plan_of(&row), None);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("registration.refunded"), false)
    );
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    let found = flow::b58(&dpp::prelude::Identifier::from([0x52u8; 32]));
    lib.w().identities.push((0, found.clone()));
    script(&lib, [(false, Ok(RegisterOutcome::PaidFailure))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(f.row(draft).identity, Some(found));
}

/// A row restored from a backup older than its create, whose identity the
/// seed already holds at its index, takes that identity and plans nothing.
#[test]
fn an_identity_found_at_the_index_is_taken_before_any_plan() {
    let f = fixture(false);
    let lib = f.fake();
    let found = flow::b58(&dpp::prelude::Identifier::from([0x53u8; 32]));
    lib.w().identities.push((0, found.clone()));
    let draft = f.insert(platform_row(None));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert!(lib.w().platform_asks.is_empty());
    let row = f.row(draft);
    assert_eq!((plan_of(&row), row.identity), (None, Some(found)));
}

/// F10: a create that can never execute clears the plan and the row plans
/// again; one that released no signature also takes its refusal's step.
#[test]
fn a_create_that_never_lands_plans_again() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    script(
        &lib,
        [
            (false, Ok(RegisterOutcome::Replan(None))),
            (
                true,
                Ok(RegisterOutcome::Replan(Some(
                    PlatformError::SignerUnavailable,
                ))),
            ),
        ],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let asks = lib.w().platform_asks.clone();
    assert_eq!(asks.len(), 2);
    assert!(asks[0].plan.is_some() && asks[1].plan.is_none());
    assert_eq!(plan_of(&f.row(draft)), None);
    assert!(!f.status(&lib, draft).funds_committed);
    assert!(matches!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::Unlock | RegistrationWait::Authorize)
    ));
}

/// F9: refusals before anything is written: the quote's numbers changed
/// (for good), a move pending (a wait); the row keeps no plan.
#[test]
fn a_refused_pass_writes_no_plan() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(platform_row(None));
    script(&lib, [(false, Err(CreditsError::QuoteChanged))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(plan_of(&row), None);
    assert_eq!(row.error.as_deref(), Some("registration.quote_changed"));
    let draft = f.insert(platform_row(None));
    script(
        &lib,
        [(
            false,
            Err(CreditsError::MovePending {
                id: "move-1-0".into(),
            }),
        )],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!((plan_of(&row), row.error), (None, None));
    assert!(f.status(&lib, draft).waiting.is_some());
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
    // The step too: a row with no plan fails, signing nothing; one with a
    // plan (another build's) waits, its plan kept.
    let draft = f.insert(platform_row(None));
    script(
        &lib,
        [(true, Ok(RegisterOutcome::Registered(platform_identity())))],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(row.error.as_deref(), Some("platform.feature_off"));
    assert_eq!((plan_of(&row), row.identity), (None, None));
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(plan_of(&f.row(draft)), Some(platform_plan_of(FUNDING)));
    assert!(lib.w().platform_asks.is_empty(), "no pass ran on mainnet");
    // A row past its create (from another build) runs no step there.
    let draft = f.insert(NewRegistration {
        identity: Some(platform_identity()),
        phase: Phase::IdentityRegistered.as_str().into(),
        ..platform_row(Some(platform_plan_of(FUNDING)))
    });
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(f.row(draft).phase, Phase::IdentityRegistered.as_str());
    // A resume refuses before its grant is redeemed: the same grant still
    // opens a lease after it.
    let draft = f.insert(platform_row(Some(platform_plan_of(FUNDING))));
    let before = f.row(draft);
    let grant = f.grant(0, 10 * COIN * 1000, Credential::None);
    let e = (f.engine)
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &draft.to_string(),
            Some(grant.clone()),
        ))
        .unwrap_err();
    assert!(off(&e), "{e:?}");
    assert_eq!(f.row(draft), before);
    (f.engine)
        .block_on(
            f.session
                .begin_lease(f.wallet, FlowKind::Registration, &[grant]),
        )
        .expect("the refused resume left its grant unredeemed");
    // Finish transfers runs none of the Platform rows.
    let rows = f.rows();
    (f.engine)
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!(f.rows(), rows);
    assert!(lib.w().platform_asks.is_empty(), "no pass ran on mainnet");
}
