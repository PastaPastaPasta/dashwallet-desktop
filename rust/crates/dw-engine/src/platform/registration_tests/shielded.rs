//! C-funding C4b-1: a Shielded-funded registration's readiness quote, its
//! denomination per protocol version and its stored kind (F14, F16, F17).

use dpp::version::v12::PROTOCOL_VERSION_12;
use platform_wallet::wallet::shielded::ShieldedNote;

use super::*;
use crate::platform::registration::{
    SHIELDED_REST_SECS, ShieldedCheck, ShieldedFacts, denomination, shielded_readiness,
};
use crate::platform::registration_flow::is_unsupported;

const DASH: u64 = 100_000_000_000;
const FALLBACK: &str = "tdash1qown0receive0address";

fn note(position: u64, value: u64) -> ShieldedNote {
    ShieldedNote {
        position,
        cmx: [position as u8; 32],
        nullifier: [position as u8 + 100; 32],
        block_height: 1,
        is_spent: false,
        value,
        note_data: Vec::new(),
    }
}

fn now() -> u64 {
    crate::events::unix_now()
}

/// A wallet whose Shielded balance can pay: Platform on, the tree scanned,
/// 1 DASH in two notes, nothing unsettled, the notes rested.
fn ready() -> ShieldedFacts {
    ShieldedFacts {
        platform_enabled: true,
        synced: true,
        notes: vec![note(0, DASH / 2), note(1, DASH / 2)],
        unsettled: false,
        latest_incoming: Some(now() - SHIELDED_REST_SECS - 60),
        fallback: FALLBACK.into(),
    }
}

fn shielded(denomination: u64) -> RegistrationRequest {
    RegistrationRequest {
        funding: RegistrationFunding::Shielded { denomination },
        ..request(PLAIN)
    }
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

fn not_ready(readiness: &registration::ShieldedReadiness) -> Vec<ShieldedCheck> {
    (readiness.checks.iter())
        .filter(|c| !c.ready)
        .map(|c| c.check)
        .collect()
}

fn version(v: u32) -> &'static PlatformVersion {
    PlatformVersion::get(v).unwrap()
}

#[test]
fn every_readiness_check_refuses_the_start_as_in_the_quote() {
    let f = fixture(false);
    let lib = f.fake();
    let resting_since = now() - 60;
    type Change = fn(&mut World, u64);
    let cases: [(Change, ShieldedCheck, Option<u64>); 4] = [
        (
            |w, _| w.shielded.notes = vec![note(0, DASH / 1000)],
            ShieldedCheck::Balance,
            None,
        ),
        (
            |w, _| w.shielded.unsettled = true,
            ShieldedCheck::NoUnsettledSpend,
            None,
        ),
        (
            |w, _| w.shielded.synced = false,
            ShieldedCheck::Synced,
            None,
        ),
        (
            |w, at| w.shielded.latest_incoming = Some(at),
            ShieldedCheck::Rested,
            Some(resting_since + SHIELDED_REST_SECS),
        ),
    ];
    for (change, check, ready_at) in cases {
        lib.w().shielded = ready();
        change(&mut lib.w(), resting_since);
        let quote = f.engine.block_on(registration::quote(&*lib, &shielded(0)));
        let readiness = quote.unwrap().readiness.unwrap();
        assert_eq!(not_ready(&readiness)[0], check, "{check:?}");
        assert_eq!(readiness.ready_at, ready_at, "{check:?}");
        let denomination = readiness.denomination.unwrap_or(0);
        let err = start(&f, &lib, shielded(denomination));
        assert!(
            matches!(err, RegistrationError::ShieldedNotReady { reason, ready_at: at }
                if reason == check && at == ready_at),
            "{check:?}: {err:?}"
        );
        assert_eq!(err.code(), "registration.shielded_not_ready");
    }
    // Not offered: the pool below its version, or Platform off.
    for change in [
        (|w: &mut World| w.protocol_version = Some(11)) as fn(&mut World),
        |w| w.shielded.platform_enabled = false,
    ] {
        lib.w().shielded = ready();
        lib.w().protocol_version = None;
        change(&mut lib.w());
        let quote = f.engine.block_on(registration::quote(&*lib, &shielded(0)));
        assert!(!not_ready(&quote.unwrap().readiness.unwrap()).is_empty());
        let err = start(&f, &lib, shielded(0));
        assert!(
            matches!(&err, RegistrationError::Platform(PlatformError::FeatureOff { feature })
                if feature == "shielded_registration"),
            "{err:?}"
        );
    }
    assert!(f.rows().is_empty());
}

#[test]
fn a_ready_start_checks_the_quoted_denomination_and_writes_nothing() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().shielded = ready();
    let quote = (f.engine.block_on(registration::quote(&*lib, &shielded(0)))).unwrap();
    let readiness = quote.readiness.clone().unwrap();
    assert!(not_ready(&readiness).is_empty());
    assert_eq!(readiness.ready_at, None);
    let denomination = readiness.denomination.unwrap();
    assert_eq!(denomination, 3 * DASH / 100, "PV13's smallest member");
    // Nothing from the Core balance; the grant covers the notes, the name
    // and the profile (F5).
    assert_eq!(
        (quote.lock_duffs, quote.total_duffs, quote.available_duffs),
        (0, 0, 0)
    );
    assert_eq!(quote.grant.max_duffs, 0);
    assert_eq!(quote.grant.max_credits, denomination + NAME_CREDITS);
    let fee = denomination - quote.remaining_credits - NAME_CREDITS;
    assert!(fee > 0 && fee < denomination / 10, "{fee}");
    let err = start(&f, &lib, shielded(denomination + 1));
    assert_eq!(err.code(), "registration.quote_changed");
    let err = start(&f, &lib, shielded(denomination));
    assert!(
        matches!(&err, RegistrationError::Platform(PlatformError::FeatureOff { feature })
            if feature == "shielded_registration"),
        "no flow step yet: {err:?}"
    );
    assert!(f.rows().is_empty());
    // The quote ignores the request's denomination.
    let again = (f.engine.block_on(registration::quote(&*lib, &shielded(7)))).unwrap();
    assert_eq!(again.readiness.unwrap().denomination, Some(denomination));
}

#[test]
fn the_denomination_follows_the_protocol_version() {
    let sets = [
        (PROTOCOL_VERSION_12, vec![10, 30, 50, 100]),
        (PROTOCOL_VERSION_13, vec![3, 10, 25, 50, 100]),
        (PROTOCOL_VERSION_14, vec![3, 10, 25, 50, 100]),
    ];
    let notes = [note(0, 2 * DASH)];
    for (v, hundredths) in sets {
        let version = version(v);
        let set = (version.drive_abci.validation_and_processing.event_constants)
            .shielded_identity_create_denominations;
        let expected: Vec<u64> = hundredths.iter().map(|h| h * DASH / 100).collect();
        assert_eq!(set, expected, "PV{v}");
        let pick = |needed| denomination(version, &notes, needed).map(|(d, _)| d);
        assert_eq!(pick(0), Some(expected[0]), "PV{v}");
        // A contested name's fund: the smallest member above it and the fee.
        let contested = 20 * DASH / 100;
        assert_eq!(
            pick(contested),
            expected.iter().copied().find(|d| *d > contested),
            "PV{v}"
        );
        assert_eq!(pick(DASH), None, "nothing covers 1 DASH and the fee");
        let (d, fee) = denomination(version, &notes, 0).unwrap();
        assert!(fee > 0 && d > fee, "PV{v}");
    }
    // Below the pool's version the set is empty: no denomination, and the
    // start is `feature_off` (above).
    assert_eq!(denomination(version(11), &notes, 0), None);
}

#[test]
fn the_fallback_is_the_wallets_own_receive_address() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().shielded = ready();
    // A caller cannot name one: the request has no such field.
    let req: RegistrationRequest = serde_json::from_value(serde_json::json!({
        "label": PLAIN, "temporary_label": null, "initial_profile": null,
        "funding": {"kind": "shielded", "denomination": 1, "fallback": "tdash1qattacker"},
    }))
    .unwrap();
    let plan = (f.engine.block_on(registration::plan(&*lib, &req))).unwrap();
    let Funding::Shielded {
        fallback, identity, ..
    } = plan.funding
    else {
        panic!("{:?}", plan.funding)
    };
    assert_eq!(fallback, FALLBACK);
    assert_eq!(identity, None);
}

#[test]
fn notes_rest_three_hours_and_restored_ones_once_synced() {
    let version = version(PROTOCOL_VERSION_13);
    let t = 1_000_000;
    let rested = |facts: &ShieldedFacts, now| {
        let r = shielded_readiness(version, facts, Some(DASH), now);
        (!not_ready(&r).contains(&ShieldedCheck::Rested), r.ready_at)
    };
    let mut facts = ShieldedFacts {
        latest_incoming: Some(t),
        ..ready()
    };
    let end = t + SHIELDED_REST_SECS;
    assert_eq!(rested(&facts, end - 1), (false, Some(end)));
    assert_eq!(rested(&facts, end), (true, None));
    facts.latest_incoming = None;
    assert_eq!(rested(&facts, t), (true, None), "restored rows, synced");
    facts.synced = false;
    assert_eq!(rested(&facts, t), (false, None), "restored rows, syncing");
}

#[test]
fn a_shielded_row_is_never_run_by_this_build() {
    let f = fixture(false);
    let mut row = core_row(PLAIN);
    row.funding = Funding::Shielded {
        denomination: 3 * DASH / 100,
        fallback: FALLBACK.into(),
        identity: None,
    }
    .encode();
    let draft = f.insert(row);
    assert!(is_unsupported(&f.row(draft)));
}
