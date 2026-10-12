//! Step 2 (IdentityCreate) under P2b's step permit (E0-04 §2a.3, F10,
//! F16): the library's ChainLock-height and IS→CL retries sign the
//! transition again inside one call, so an earlier attempt may execute
//! however the call ends. A call that released a signature and failed
//! reads `MaybeSent`; one whose signer met the lock parks the flow; the
//! next run looks for the lock's identity before it signs again, so one
//! lock never pays for two identities (the fake asserts it).

use super::*;
use crate::platform::lease::CallStepStatus;
use crate::platform::lease::table::LeaseState;

/// Starts a registration whose identity calls meet `script`, and runs it
/// until it stops.
fn started(f: &Fixture, script: &[IdentityAttempt]) -> (Arc<Fake>, i64) {
    let lib = f.fake();
    lib.w().identity_script = script.iter().copied().collect();
    let grant = f.grant(COIN, 10 * COIN * 1000, Credential::None);
    let draft = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request(PLAIN),
            grant,
        ))
        .unwrap();
    let draft: i64 = draft.parse().unwrap();
    f.settle(draft);
    (lib, draft)
}

/// The identity step's marker, as P2b reads it.
fn identity_call(f: &Fixture, draft: i64) -> CallStepStatus {
    (f.session.leases)
        .call_step(f.wallet, &flow::identity_step(draft))
        .status
}

fn regrant(f: &Fixture, lib: &Arc<Fake>, draft: i64) {
    let grant = f.grant(COIN, 10 * COIN * 1000, Credential::None);
    resume(f, lib, draft, Some(grant));
    f.settle(draft);
}

fn done(f: &Fixture, draft: i64) {
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error.as_deref()), (Phase::Done, None));
}

/// The height retry's re-sign meets a locked signer after the first
/// attempt was released: `MaybeSent`, and the flow parks for an authority
/// before the identity exists. Granted again, the next call creates it
/// once.
#[test]
fn step2_a_locked_resign_after_a_released_attempt_is_maybe_sent_and_parks() {
    let f = fixture(false);
    let (lib, draft) = started(
        &f,
        &[IdentityAttempt::ReleasedThenLocked { executes: false }],
    );
    let status = f.status(&lib, draft);
    assert_eq!(status.waiting, Some(flow::authority_wait(&f.session)));
    assert_eq!(status.failure, None);
    assert!(status.identity.is_none());
    assert_eq!(
        identity_call(&f, draft),
        CallStepStatus::Marked { durable: true },
        "MaybeSent: the marker stands"
    );
    assert_eq!(lib.counts(), (1, 0));
    regrant(&f, &lib, draft);
    done(&f, draft);
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(identity_call(&f, draft), CallStepStatus::Sent);
}

/// The parked flow's first attempt lands after all: the next run finds the
/// lock's identity and signs nothing more (a second IdentityCreate from
/// the lock panics in the fake).
#[test]
fn step2_a_parked_flow_whose_first_attempt_landed_signs_nothing_more() {
    let f = fixture(false);
    let (lib, draft) = started(
        &f,
        &[IdentityAttempt::ReleasedThenLocked { executes: false }],
    );
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(flow::authority_wait(&f.session))
    );
    let row = f.row(draft);
    let (outpoint, index) = (
        flow::outpoint_of(&row).unwrap(),
        row.identity_index.unwrap(),
    );
    lib.create_identity(outpoint, index);
    regrant(&f, &lib, draft);
    done(&f, draft);
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(
        f.row(draft).identity,
        Some(flow::b58(&flow::identity_for(outpoint)))
    );
}

/// The first attempt landed before the re-sign met the lock: the call
/// fails, the identity is found at once and recorded, and the flow parks
/// only at the names step, whose signer the lock stopped too. Granted
/// again, it ends without another IdentityCreate.
#[test]
fn step2_a_landed_attempt_behind_a_locked_resign_records_the_identity() {
    let f = fixture(false);
    let (lib, draft) = started(
        &f,
        &[IdentityAttempt::ReleasedThenLocked { executes: true }],
    );
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::NameRequested, None)
    );
    let outpoint = flow::outpoint_of(&row).unwrap();
    assert_eq!(row.identity, Some(flow::b58(&flow::identity_for(outpoint))));
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(flow::authority_wait(&f.session))
    );
    assert_eq!(
        identity_call(&f, draft),
        CallStepStatus::Marked { durable: true },
        "the call itself failed after a release: MaybeSent"
    );
    regrant(&f, &lib, draft);
    done(&f, draft);
    assert_eq!(lib.counts(), (1, 1));
}

/// Platform reaches the lock's height at the second attempt, inside the
/// one call: created, `Sent`.
#[test]
fn step2_a_height_retry_inside_the_call_creates_once() {
    let f = fixture(false);
    let (lib, draft) = started(&f, &[IdentityAttempt::RetriedThenCreated]);
    done(&f, draft);
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(identity_call(&f, draft), CallStepStatus::Sent);
}

/// The library gives up on the height with every attempt released and
/// none landed: `MaybeSent`, and no lock met, so the run stops to wait for
/// the network with no failure recorded and its lease still signing; the
/// next run creates the identity once. A running flow retries on its own.
#[test]
fn step2_retries_that_give_up_wait_for_the_network_and_go_on() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().identity_script = [IdentityAttempt::ReleasedThenUnavailable].into();
    let draft = f.insert(core_row(PLAIN));
    let lease = f.lease();
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert!(row.identity.is_none());
    assert_eq!(row.error, None, "{row:?}");
    assert_eq!(
        identity_call(&f, draft),
        CallStepStatus::Marked { durable: true }
    );
    assert_eq!(lease.state(), Some(LeaseState::Active), "not parked");
    assert_eq!(lib.counts(), (1, 0));
    f.drive(&lib, draft, Some(lease), false);
    done(&f, draft);
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(identity_call(&f, draft), CallStepStatus::Sent);

    let lib = f.fake();
    lib.w().identity_script = [IdentityAttempt::ReleasedThenUnavailable].into();
    let draft = f.insert(core_row("frank2"));
    f.spawn(&lib, draft, Some(f.lease()));
    f.settle(draft);
    done(&f, draft);
    assert_eq!(lib.counts(), (1, 1));
}
