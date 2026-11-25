//! C-funding C4b-1: a Shielded-funded registration's readiness quote, its
//! denomination per protocol version and its stored kind (F14, F16, F17).

use dpp::version::v12::PROTOCOL_VERSION_12;
use platform_wallet::wallet::shielded::ShieldedNote;

use super::*;
use crate::platform::registration::RegistrationWait;
use crate::platform::registration::{
    SHIELDED_REST_SECS, ShieldedCheck, ShieldedFacts, denomination, shielded_readiness,
};
use crate::platform::registration_flow::is_unsupported;
use crate::platform::registration_shielded::ShieldedOutcome;

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
        rest_known: true,
        latest_incoming: Some(now() - SHIELDED_REST_SECS - 60),
        pool_notes: None,
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
    assert_eq!(
        fee,
        crate::platform::registration_flow::IDENTITY_CREATE_CREDITS,
        "the Core path's bound"
    );
    let err = start(&f, &lib, shielded(denomination + 1));
    assert_eq!(err.code(), "registration.quote_changed");
    // Past every check, the start redeems its grant: an unknown one is
    // refused before anything is written.
    let err = start(&f, &lib, shielded(denomination));
    assert_eq!(
        err,
        RegistrationError::Platform(PlatformError::GrantInvalid)
    );
    assert!(f.rows().is_empty());
    // Each fallback is kept from every later hand-out, so a refused start
    // takes none (C-funding F13).
    assert_eq!(lib.w().fallbacks_taken, 0);
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
fn a_caller_cannot_name_the_fallback() {
    let req: RegistrationRequest = serde_json::from_value(serde_json::json!({
        "label": PLAIN, "temporary_label": null, "initial_profile": null,
        "funding": {"kind": "shielded", "denomination": 1, "fallback": "tdash1qattacker"},
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&req.funding).unwrap(),
        serde_json::json!({"kind": "shielded", "denomination": 1})
    );
}

#[test]
fn notes_rest_three_hours_restored_ones_once_synced_and_none_unknown() {
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
    facts.rest_known = false;
    assert_eq!(rested(&facts, end - 1), (false, None), "the clock unknown");
    facts.rest_known = true;
    facts.latest_incoming = None;
    assert_eq!(rested(&facts, t), (true, None), "restored rows, synced");
    facts.synced = false;
    assert_eq!(rested(&facts, t), (false, None), "restored rows, syncing");
}

const DENOMINATION: u64 = 3 * DASH / 100;

fn shielded_row(identity: Option<String>) -> NewRegistration {
    NewRegistration {
        identity_index: Some(0),
        funding: Funding::Shielded {
            denomination: DENOMINATION,
            fallback: FALLBACK.into(),
            identity,
        }
        .encode(),
        phase: Phase::KeysPrepared.as_str().into(),
        ..core_row(PLAIN)
    }
}

/// The identity the row's Shielded funding names.
fn named(row: &RegistrationRow) -> Option<String> {
    match Funding::decode(&row.funding) {
        crate::platform::registration_rows::Decoded::Known(Funding::Shielded {
            identity, ..
        }) => identity,
        other => panic!("{other:?}"),
    }
}

fn created() -> String {
    flow::b58(&dpp::prelude::Identifier::from([0x61u8; 32]))
}

fn script(
    lib: &Fake,
    passes: impl IntoIterator<Item = (Option<String>, Result<ShieldedOutcome, PlatformError>)>,
) {
    lib.w().shielded_fund.extend(passes);
}

/// F5, F13: a ready start under its grant writes a row funded from the
/// Shielded balance at the quoted denomination, its fallback the wallet's
/// own Platform receive address as the start reads it, no identity named.
#[test]
fn a_ready_start_writes_the_row_with_its_own_fallback() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().shielded = ready();
    let quote = (f.engine.block_on(registration::quote(&*lib, &shielded(0)))).unwrap();
    let denomination = quote.readiness.unwrap().denomination.unwrap();
    assert_eq!(quote.grant.max_duffs, 0);
    let g = f.grant(0, quote.grant.max_credits, f.credential());
    (f.engine)
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            shielded(denomination),
            g,
        ))
        .unwrap();
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        Funding::decode(&rows[0].funding),
        crate::platform::registration_rows::Decoded::Known(Funding::Shielded {
            denomination,
            fallback: super::FALLBACK.into(),
            identity: None,
        })
    );
}

/// F13: a start refused because another registration's payment is still
/// open takes no fallback, so a refused start widens no gap of the
/// Platform payment account.
#[test]
fn a_start_refused_in_progress_takes_no_fallback() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().shielded = ready();
    let open = f.insert(shielded_row(Some(created())));
    let quote = (f.engine.block_on(registration::quote(&*lib, &shielded(0)))).unwrap();
    let denomination = quote.readiness.unwrap().denomination.unwrap();
    let g = f.grant(0, quote.grant.max_credits, f.credential());
    let start = (f.engine).block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        shielded(denomination),
        g,
    ));
    assert!(
        matches!(&start, Err(RegistrationError::InProgress { draft }) if *draft == open.to_string()),
        "{start:?}"
    );
    assert_eq!(f.rows().len(), 1);
    assert_eq!(lib.w().fallbacks_taken, 0);
}

/// Mainnet: off (flag `shielded_registration`) until a payment that stays
/// unknown has a way out (C-funding C3). The quote and the start are
/// refused before any name is looked up (DAPI is down), the grant redeemed,
/// a fallback taken or a row written; an unnamed row's resume before its
/// grant is redeemed. A named row is not refused: it settles by evidence.
#[test]
fn mainnet_refuses_a_shielded_registration_before_anything_is_written() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().shielded = ready();
    let quote = (f.engine.block_on(registration::quote(&*lib, &shielded(0)))).unwrap();
    let denomination = quote.readiness.unwrap().denomination.unwrap();
    lib.w().mainnet = true;
    lib.w().names_offline = true;
    let off = RegistrationError::Platform(PlatformError::FeatureOff {
        feature: "shielded_registration".into(),
    });
    let e = (f.engine.block_on(registration::quote(&*lib, &shielded(0)))).unwrap_err();
    assert_eq!(e, off);
    let g = f.grant(0, quote.grant.max_credits, f.credential());
    let e = (f.engine)
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            shielded(denomination),
            g,
        ))
        .unwrap_err();
    assert_eq!(e, off);
    assert!(f.rows().is_empty());
    assert_eq!(lib.w().fallbacks_taken, 0);
    lib.w().names_offline = false;
    let unnamed = f.insert(shielded_row(None));
    let before = f.row(unnamed);
    let g = f.grant(0, quote.grant.max_credits, f.credential());
    let e = (f.engine)
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &unnamed.to_string(),
            Some(g),
        ))
        .unwrap_err();
    assert_eq!(e, off);
    assert_eq!(f.row(unnamed), before);
    assert!(lib.w().shielded_asks.is_empty());
    let named = f.insert(shielded_row(Some(created())));
    let resumed = (f.engine).block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        &named.to_string(),
        None,
    ));
    assert_ne!(resumed, Err(off.clone()));
    let named = f.insert(shielded_row(Some(created())));
    let g = f.grant(0, quote.grant.max_credits, f.credential());
    let resumed = (f.engine).block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        &named.to_string(),
        Some(g),
    ));
    assert_ne!(resumed, Err(off));
}

/// On mainnet a named row still settles by its evidence, with a lease or
/// without one; an unnamed row the library refuses as `feature_off` fails
/// retryable and stays unnamed, its funds uncommitted.
#[test]
fn on_mainnet_a_named_row_settles_and_an_unnamed_one_signs_nothing() {
    for with_lease in [false, true] {
        let f = fixture(false);
        let lib = f.fake();
        lib.w().mainnet = true;
        let id = created();
        let draft = f.insert(shielded_row(Some(id.clone())));
        script(&lib, [(None, Ok(ShieldedOutcome::Registered(id.clone())))]);
        f.drive(&lib, draft, with_lease.then(|| f.lease()), false);
        let row = f.row(draft);
        assert_eq!(row.identity.as_deref(), Some(id.as_str()), "{with_lease}");
        assert!(phase(&row) > Phase::IdentityRegistered, "{}", row.phase);
        let asks = lib.w().shielded_asks.clone();
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].identity, Some(id), "evidence, not a create");
    }
    let f = fixture(false);
    let lib = f.fake();
    lib.w().mainnet = true;
    let draft = f.insert(shielded_row(None));
    let off = PlatformError::FeatureOff {
        feature: "shielded_registration".into(),
    };
    script(&lib, [(None, Err(off))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable, named(&row)),
        (Some("platform.feature_off"), true, None)
    );
    assert_eq!(phase(&row), Phase::KeysPrepared);
    assert!(!f.status(&lib, draft).funds_committed);
}

/// C4b-2: this build reads and runs a Shielded row, named or not.
#[test]
fn a_shielded_row_is_run_by_this_build() {
    let f = fixture(false);
    for identity in [None, Some(created())] {
        let draft = f.insert(shielded_row(identity));
        assert!(!is_unsupported(&f.row(draft)));
    }
}

/// F11: the create the hook named and Platform proved is the row's
/// identity, and the flow goes on to its name; the pass carries the row's
/// claim, slot, denomination and fallback, and no identity.
#[test]
fn a_proved_create_registers_the_named_identity() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(shielded_row(None));
    let id = created();
    script(
        &lib,
        [(
            Some(id.clone()),
            Ok(ShieldedOutcome::Registered(id.clone())),
        )],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(row.identity.as_deref(), Some(id.as_str()));
    assert!(phase(&row) > Phase::IdentityRegistered, "{}", row.phase);
    assert_eq!(named(&row), Some(id), "the name stays");
    let asks = lib.w().shielded_asks.clone();
    assert_eq!(asks.len(), 1);
    let ask = &asks[0];
    assert_eq!(
        (
            ask.draft,
            ask.index,
            ask.denomination,
            ask.fallback.as_str(),
            ask.identity.clone()
        ),
        (draft, 0, DENOMINATION, FALLBACK, None)
    );
}

/// F12: a named create of unknown outcome waits `payment_unknown`: its
/// funds are committed and the row is not discarded. The next pass hands
/// the name back for evidence, without a lease, and creates nothing new.
#[test]
fn an_unknown_create_waits_and_refuses_the_discard() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(shielded_row(None));
    let id = created();
    script(&lib, [(Some(id.clone()), Ok(ShieldedOutcome::Unknown))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), named(&row)),
        (Phase::KeysPrepared, Some(id.clone()))
    );
    let status = f.status(&lib, draft);
    assert!(status.funds_committed);
    assert_eq!(status.waiting, Some(RegistrationWait::PaymentUnknown));
    assert!(
        f.discard(&lib, draft).is_err(),
        "a named create is not discarded"
    );
    script(&lib, [(None, Ok(ShieldedOutcome::Registered(id.clone())))]);
    f.drive(&lib, draft, None, false);
    let asks = lib.w().shielded_asks.clone();
    assert_eq!(asks.len(), 2);
    assert_eq!(asks[1].identity, Some(id.clone()), "evidence, not a create");
    assert_eq!(f.row(draft).identity, Some(id));
}

/// F12: a named row the app reopened on has no run: it reads
/// `payment_unknown`, not running. A resume with no grant runs evidence
/// alone (the name handed back, no lease) and a now-proved identity
/// registers it.
#[test]
fn a_reopened_named_row_is_checked_again_by_a_resume_without_a_grant() {
    let f = fixture(false);
    let lib = f.fake();
    let id = created();
    let draft = f.insert(shielded_row(Some(id.clone())));
    let status = f.status(&lib, draft);
    assert_eq!(
        (status.waiting, status.running),
        (Some(RegistrationWait::PaymentUnknown), false)
    );
    script(&lib, [(None, Ok(ShieldedOutcome::Registered(id.clone())))]);
    super::resume(&f, &lib, draft, None);
    eventually("the evidence registers the row", || {
        f.row(draft).identity.as_deref() == Some(id.as_str())
    });
    let asks = lib.w().shielded_asks.clone();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].identity, Some(id), "evidence, not a create");
}

/// F12, F13: Platform's fallback: an identity of the seed at the row's
/// index is the row's, but never the one the create named, which was just
/// proved absent however the library recorded it; else
/// `registration.refunded`, not retryable, the name cleared and the row
/// free to discard.
#[test]
fn a_fallback_adopts_the_index_identity_or_fails_refunded() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(shielded_row(Some(created())));
    lib.w().identities.push((0, created()));
    script(&lib, [(None, Ok(ShieldedOutcome::Fallback(created())))]);
    f.drive(&lib, draft, None, false);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable, named(&row)),
        (Some("registration.refunded"), false, None)
    );
    assert!(!f.status(&lib, draft).funds_committed);
    f.discard(&lib, draft).unwrap();
    let draft = f.insert(shielded_row(Some(created())));
    let found = flow::b58(&dpp::prelude::Identifier::from([0x62u8; 32]));
    lib.w().identities = vec![(0, found.clone())];
    script(&lib, [(None, Ok(ShieldedOutcome::Fallback(created())))]);
    f.drive(&lib, draft, None, false);
    assert_eq!(f.row(draft).identity, Some(found));
}

/// F12: a create of two or more notes whose notes were spent with no
/// identity adopts an identity of the seed at the row's index as a
/// fallback does, else fails `registration.payment_consumed`, not
/// retryable and claiming no refund, the name cleared and the row free to
/// discard.
#[test]
fn consumed_notes_adopt_the_index_identity_or_fail_consumed() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(shielded_row(Some(created())));
    lib.w().identities.push((0, created()));
    script(&lib, [(None, Ok(ShieldedOutcome::Consumed(created())))]);
    f.drive(&lib, draft, None, false);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable, named(&row)),
        (Some("registration.payment_consumed"), false, None)
    );
    assert!(!f.status(&lib, draft).funds_committed);
    f.discard(&lib, draft).unwrap();
    let draft = f.insert(shielded_row(Some(created())));
    let found = flow::b58(&dpp::prelude::Identifier::from([0x62u8; 32]));
    lib.w().identities = vec![(0, found.clone())];
    script(&lib, [(None, Ok(ShieldedOutcome::Consumed(created())))]);
    f.drive(&lib, draft, None, false);
    assert_eq!(f.row(draft).identity, Some(found));
}

/// F12, F13: Platform's fallback on the pass that created: the library
/// recorded the identity the create named at the row's index although it
/// was proved absent, so it is not adopted and the row fails refunded.
#[test]
fn a_fresh_fallback_never_adopts_the_identity_the_create_named() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().shielded = ready();
    lib.w().shielded_records_named = true;
    let draft = f.insert(shielded_row(None));
    script(
        &lib,
        [(Some(created()), Ok(ShieldedOutcome::Fallback(created())))],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable, row.identity.as_deref()),
        (Some("registration.refunded"), false, None)
    );
}

/// F12: a named create that never reached the journal clears its name and
/// the row creates again in the same run; one whose artifact reads not
/// executed clears its name and fails, retryable, so nothing loops.
#[test]
fn an_unsent_create_is_cleared_and_never_loops() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(shielded_row(Some(created())));
    let again = flow::b58(&dpp::prelude::Identifier::from([0x63u8; 32]));
    script(
        &lib,
        [
            (None, Ok(ShieldedOutcome::Cleared)),
            (
                Some(again.clone()),
                Ok(ShieldedOutcome::Registered(again.clone())),
            ),
        ],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let asks = lib.w().shielded_asks.clone();
    assert_eq!(
        asks.iter().map(|a| a.identity.clone()).collect::<Vec<_>>(),
        [Some(created()), None]
    );
    assert_eq!(f.row(draft).identity, Some(again));
    let draft = f.insert(shielded_row(Some(created())));
    script(&lib, [(None, Ok(ShieldedOutcome::Unsent))]);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable, named(&row)),
        (Some("shielded.spend_refused"), true, None)
    );
    assert_eq!(lib.w().shielded_asks.len(), 3, "no second pass");
}

/// F16: a check that no longer passes fails the row, retryable, nothing
/// named; without a lease an unnamed row parks for one and asks nothing;
/// an identity at the row's index is taken before any create.
#[test]
fn a_create_needs_readiness_a_lease_and_a_free_index() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(shielded_row(None));
    script(
        &lib,
        [(
            None,
            Ok(ShieldedOutcome::NotReady(ShieldedCheck::PoolSize, None)),
        )],
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable, named(&row)),
        (Some("registration.shielded_not_ready"), true, None)
    );
    let draft = f.insert(shielded_row(None));
    f.drive(&lib, draft, None, false);
    assert_eq!(lib.w().shielded_asks.len(), 1, "no pass without a lease");
    assert!(matches!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::Unlock | RegistrationWait::Authorize)
    ));
    let found = flow::b58(&dpp::prelude::Identifier::from([0x64u8; 32]));
    lib.w().identities.push((0, found.clone()));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(lib.w().shielded_asks.len(), 1);
    assert_eq!(f.row(draft).identity, Some(found));
}

/// What a Shielded-funded registration spends for a name, at PV13 and
/// PV14, with one 0.1 DASH note: (denomination, fee, credits left after
/// the names and the profile). Platform's real name fees.
fn pinned(v: u32, label: &str, temporary: Option<&str>) -> (u64, u64, u64) {
    use crate::platform::names::{name_cost, valid_label};
    let version = version(v);
    let cost = |label: &str| name_cost(&valid_label(label).unwrap(), 0, version).total;
    let needed = cost(label) + temporary.map_or(0, cost) + flow::PROFILE_CREDITS;
    let notes = [note(0, DASH / 10)];
    let (d, fee) = denomination(version, &notes, needed).unwrap();
    (d, fee, d - fee - needed)
}

/// The denomination the quote pins for a plain name, and for a contested
/// one with its temporary name, both from Shielded: the smallest member
/// whose value less the create's fee covers every name and the profile
/// (F14), so a 0.1 note pays 0.03 for a plain name and keeps 0.07 as
/// change, and a contested name with a temporary one takes 0.25 (more than
/// the note: refused as Balance, never a smaller one).
#[test]
fn the_pinned_denominations_cover_every_name_and_leave_credits() {
    for v in [PROTOCOL_VERSION_13, PROTOCOL_VERSION_14] {
        let (d, fee, left) = pinned(v, PLAIN, None);
        assert_eq!(d, 3 * DASH / 100, "PV{v} plain");
        assert!(left > 0 && fee > 0, "PV{v} plain: {fee} fee, {left} left");
        let version = version(v);
        let cost = |label: &str| {
            use crate::platform::names::{name_cost, valid_label};
            name_cost(&valid_label(label).unwrap(), 0, version).total
        };
        let notes = [note(0, DASH)];
        let both = cost(CONTESTED) + cost(PLAIN) + flow::PROFILE_CREDITS;
        let alone = cost(CONTESTED) + flow::PROFILE_CREDITS;
        let (d_both, fee_both) = denomination(version, &notes, both).unwrap();
        let (d_alone, _) = denomination(version, &notes, alone).unwrap();
        assert_eq!(d_both, 25 * DASH / 100, "PV{v} contested + temporary");
        assert!(d_both - fee_both >= both, "PV{v}: both names covered");
        assert!(d_both >= d_alone);
        println!(
            "PV{v}: plain {d} (fee {fee}, left {left}); contested+temporary {d_both} \
             (fee {fee_both}, left {})",
            d_both - fee_both - both
        );
    }
}

/// F20: the confirm's change line. The create selects its notes as the
/// library does and pays what they hold above the denomination back to the
/// wallet's own default shielded address, so a 0.1 note paying a plain name
/// returns 0.07 to this wallet's Shielded balance, and an exact note none.
#[test]
fn the_change_of_a_create_returns_to_this_wallets_shielded_balance() {
    for v in [PROTOCOL_VERSION_13, PROTOCOL_VERSION_14] {
        let version = version(v);
        let d = 3 * DASH / 100;
        let readiness = |notes: Vec<ShieldedNote>| {
            let facts = ShieldedFacts {
                notes,
                ..ShieldedFacts::default()
            };
            shielded_readiness(version, &facts, Some(d), 0)
        };
        assert_eq!(
            readiness(vec![note(0, DASH / 10)]).change,
            Some(7 * DASH / 100),
            "PV{v}"
        );
        assert_eq!(readiness(vec![note(0, d)]).change, Some(0), "PV{v}");
        assert_eq!(
            readiness(vec![note(0, d / 2)]).change,
            None,
            "PV{v}: cannot pay"
        );
        assert_eq!(
            shielded_readiness(version, &ShieldedFacts::default(), None, 0).change,
            None
        );
    }
}

/// The repin rule and F11: one function of the engine names the library's
/// pool-funded identity create, `PwShielded::identity_create`, and it
/// names the keyset create (through the dispatch hook), never the seed
/// path nor the dispatched create underneath. Mutant: route through the
/// seed `shielded_identity_create_from_pool`.
#[test]
fn the_keyset_create_is_the_engines_only_call_of_the_library_create() {
    fn sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.to_string_lossy().into_owned();
            if path.is_dir() {
                if !name.ends_with("_tests") {
                    sources(&path, out);
                }
            } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
                out.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let calls: Vec<(String, String)> = (files.iter())
        .flat_map(|(name, text)| {
            text.match_indices(".shielded_identity_create_from_pool")
                .map(move |(at, _)| (name.clone(), text[at..].chars().take(60).collect()))
        })
        .collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls[0].0.ends_with("platform/shielded_pool.rs"),
        "{calls:?}"
    );
    assert!(
        calls[0]
            .1
            .starts_with(".shielded_identity_create_from_pool_with_keyset("),
        "{calls:?}"
    );
    assert!(
        !files
            .iter()
            .any(|(_, text)| text.contains("identity_create_from_shielded_pool_dispatched")),
        "the dispatched create is the library's to call"
    );
}
