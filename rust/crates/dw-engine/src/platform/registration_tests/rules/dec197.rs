//! DEC-197 (Q15): a start past a wedged step, the step a released funding
//! call left with no recorded lock (`MaybeSent` for good, Repair refuses
//! it). The override needs SPV synced and the release an hour old (the
//! release time stood in for through [`Views`]); the old row stays
//! committed. Nothing adopts the old lock (DEC-198): if it shows up, only
//! Recovery may take it, and Finish does.

use super::r8::{Marker, start, unlanded};
use super::*;
use crate::platform::registration_ledger::test_views::Views;
use crate::platform::registration_ledger::{IndexOwner, Ledger, Owner, Taker};

fn start_past(f: &Fixture, lib: &Arc<Fake>, past: &[String]) -> Result<String, RegistrationError> {
    f.engine.block_on(registration::start_past(
        &f.session,
        f.wallet,
        Arc::clone(lib),
        request("replace2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
        past,
    ))
}

/// The wedge: A's released step with no recorded lock, and the lock its
/// call signed (`original`), which may be on the wire.
fn wedged(f: &mut Fixture) -> (i64, OutPoint, Arc<Fake>) {
    let (draft, original) = unlanded(f, Marker::Released);
    let lib = f.fake();
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::FundingUnknown)
    );
    (draft, original, lib)
}

const HOUR: u64 = registration::PAST_UNKNOWN_FUNDING_AFTER;

/// The override is refused while the release is under an hour old, with
/// the wait left, and while no release time is known (P2b r7 fills the
/// seam; until then the override fails closed). A stays as it was.
#[test]
fn dec197_the_override_waits_an_hour_after_the_release() {
    let mut f = fixture(false);
    let (a, _, lib) = wedged(&mut f);
    let past = [a.to_string()];
    let unknown = start_past(&f, &lib, &past);
    assert!(
        matches!(
            &unknown,
            Err(RegistrationError::FundingUnknownRecent { draft, wait_secs: None })
                if *draft == a.to_string()
        ),
        "{unknown:?}"
    );
    let views = Views::new(&f.session.leases, f.wallet);
    views.set_released(a, crate::events::unix_now() - 60);
    let recent = start_past(&f, &lib, &past);
    assert!(
        matches!(
            &recent,
            Err(RegistrationError::FundingUnknownRecent { draft, wait_secs: Some(w) })
                if *draft == a.to_string() && (HOUR - 120..=HOUR - 60).contains(w)
        ),
        "{recent:?}"
    );
    assert_eq!(
        recent.unwrap_err().code(),
        "registration.funding_unknown_recent"
    );
    assert_eq!(f.rows().len(), 1);
    assert_eq!(lib.counts(), (0, 0));
}

/// The override is refused while SPV is behind, however old the release.
#[test]
fn dec197_the_override_waits_for_sync() {
    let mut f = fixture(false);
    let (a, _, lib) = wedged(&mut f);
    let views = Views::new(&f.session.leases, f.wallet);
    views.set_released(a, crate::events::unix_now() - 2 * HOUR);
    lib.w().unsynced = true;
    let refused = start_past(&f, &lib, &[format!("registration/{a}/funding")]);
    assert!(
        matches!(refused, Err(RegistrationError::NotSynced)),
        "{refused:?}"
    );
    assert_eq!(f.rows().len(), 1);
}

/// Synced and an hour on, the override starts C at a fresh index, funds
/// and completes it. A stays committed: discard is refused, its step stays
/// `Unknown`, and a start that does not name it is still refused.
#[test]
fn dec197_the_override_starts_past_a_wedged_step() {
    let mut f = fixture(false);
    let (a, _, lib) = wedged(&mut f);
    let index = f.row(a).identity_index;
    let views = Views::new(&f.session.leases, f.wallet);
    views.set_released(a, crate::events::unix_now() - HOUR);
    assert!(matches!(
        start(&f, &lib),
        Err(RegistrationError::InProgress { draft }) if draft == a.to_string()
    ));
    let c: i64 = start_past(&f, &lib, &[a.to_string()])
        .unwrap()
        .parse()
        .unwrap();
    f.settle(c);
    let row = f.row(c);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::Done, None),
        "{row:?}"
    );
    assert_ne!(row.identity_index, index, "C funded on A's slot");
    assert_eq!(lib.counts(), (1, 1));

    let status = f.status(&lib, a);
    assert!(status.funds_committed);
    assert_eq!(status.waiting, Some(RegistrationWait::FundingUnknown));
    assert_eq!(f.discard(&lib, a).unwrap_err().code(), "invalid_argument");
    assert!(matches!(
        start(&f, &lib),
        Err(RegistrationError::InProgress { draft }) if draft == a.to_string()
    ));
}

/// The earlier attempt's lock shows up after the override, rebuilt from the
/// chain with its index unknown, while C is open and unfunded (its funding
/// failed before anything was signed). Only A's wedged step contests it, so
/// no row may take it, C included, and C's index reads `Unknown`: resumed,
/// C waits `NeedsFinish`. Finish recovers it into a recovery row (DEC-198
/// am.1: the funds come back as a lock the wallet recovers, never to A or
/// C); then C funds and completes on its own lock, and A stays waiting,
/// committed.
#[test]
fn dec197_the_earlier_lock_showing_up_is_recovered_by_finish() {
    let mut f = fixture(false);
    let (a, original, lib) = wedged(&mut f);
    let views = Views::new(&f.session.leases, f.wallet);
    views.set_released(a, crate::events::unix_now() - HOUR);
    lib.w().fund_script = Some(FundScript::FailUntracked);
    let c: i64 = start_past(&f, &lib, &[a.to_string()])
        .unwrap()
        .parse()
        .unwrap();
    f.settle(c);
    let row = f.row(c);
    assert_eq!(
        (flow::outpoint_of(&row), row.retryable),
        (None, true),
        "{row:?}"
    );
    lib.w().fund_script = None;
    let lock = registration_lock(original, 0, LockStatus::RecoveredFromChain);
    lib.w().locks.push(lock.clone());
    let l = f
        .engine
        .block_on(Ledger::load(&f.session, f.wallet, &*lib))
        .unwrap();
    assert_eq!(l.owner(&lock), Some(Owner::Contested));
    assert!(!l.may_take(Taker::Row(c), &lock));
    assert!(l.may_take(Taker::Recovery, &lock));
    assert_eq!(
        l.index_owner(&row, row.identity_index.unwrap()),
        IndexOwner::Unknown
    );
    assert_eq!(l.pending(), 2);
    assert_eq!(l.unattributed_wait(&row), RegistrationWait::NeedsFinish);

    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &c.to_string(),
            Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
        ))
        .unwrap();
    eventually("C waits for Finish", || {
        f.status(&lib, c).waiting == Some(RegistrationWait::NeedsFinish)
    });
    let report = finish_once(&f, &lib);
    assert_eq!(report.still_pending, 1, "{report:?}");
    let holders = held_by(&f, original);
    assert_eq!(holders.len(), 1);
    assert!(holders != [a] && holders != [c], "{holders:?}");
    assert_eq!(f.rows().len(), 3);
    f.settle(c);
    let row = f.row(c);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::Done, None),
        "{row:?}"
    );
    assert_ne!(flow::outpoint_of(&row), Some(original));
    assert_eq!(
        f.status(&lib, a).waiting,
        Some(RegistrationWait::FundingUnknown)
    );
    assert_eq!(f.discard(&lib, a).unwrap_err().code(), "invalid_argument");
}

/// The override past an orphan step (its row removed), named as
/// `registration.in_progress` names it: C starts and completes, and the
/// orphan stays listed, outstanding.
#[test]
fn dec197_the_override_passes_a_wedged_orphan_step() {
    let mut f = fixture(false);
    let (a, _, lib) = wedged(&mut f);
    let row = f.row(a);
    assert!(
        f.appdb()
            .delete_registration(&f.wallet.to_string(), a, row.owner, &row.phase, None)
            .unwrap()
    );
    let step = flow::funding_step(a);
    assert!(matches!(
        start(&f, &lib),
        Err(RegistrationError::InProgress { draft }) if draft == step
    ));
    let views = Views::new(&f.session.leases, f.wallet);
    views.set_released(a, crate::events::unix_now() - HOUR);
    let c: i64 = start_past(&f, &lib, std::slice::from_ref(&step))
        .unwrap()
        .parse()
        .unwrap();
    f.settle(c);
    assert_eq!(phase(&f.row(c)), Phase::Done);
    let orphan = (f.statuses(&lib).into_iter())
        .find(|s| s.draft == step)
        .expect("the orphan step is listed");
    assert_eq!(orphan.waiting, Some(RegistrationWait::FundingUnknown));
}

/// Only a wedged step is passed: an unflagged marker's `Unknown` (§8.1) and
/// a row holding its lock still block, named or not. Naming a draft that
/// blocks nothing is harmless; a name that is no registration is refused.
#[test]
fn dec197_the_override_passes_only_wedged_steps() {
    let mut f = fixture(false);
    let (a, _, lib) = {
        let (draft, original) = unlanded(&mut f, Marker::Unflagged);
        (draft, original, f.fake())
    };
    let views = Views::new(&f.session.leases, f.wallet);
    views.set_released(a, crate::events::unix_now() - 2 * HOUR);
    assert!(matches!(
        start_past(&f, &lib, &[a.to_string()]),
        Err(RegistrationError::InProgress { draft }) if draft == a.to_string()
    ));
    drop(views);

    let f = fixture(false);
    let lib = f.fake();
    // Broadcast, no proof yet: B waits at `ProofWaiting` holding its lock.
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().proof_arrives = false;
    let b = f.insert(core_row(PLAIN));
    f.drive(&lib, b, Some(f.lease()), false);
    assert_eq!(phase(&f.row(b)), Phase::ProofWaiting);
    assert!(matches!(
        start_past(&f, &lib, &[b.to_string()]),
        Err(RegistrationError::InProgress { draft }) if draft == b.to_string()
    ));
    assert_eq!(
        start_past(&f, &lib, &["registration/x/funding".into()])
            .unwrap_err()
            .code(),
        "invalid_argument"
    );

    let f = fixture(false);
    let lib = f.fake();
    let c: i64 = start_past(&f, &lib, &["999".into()])
        .unwrap()
        .parse()
        .unwrap();
    f.settle(c);
    assert_eq!(phase(&f.row(c)), Phase::Done);
}
