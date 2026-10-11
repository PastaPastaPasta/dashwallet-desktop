//! DP1-02 §11.2: the r8 findings, the restore orphan and heir (Q5, Q7), and
//! an `Ok` that is not the step's own lock (I3), on the real lease table,
//! journal and SQLite with the Fake library.

use super::*;

/// A restart: the session closes and opens again, and P2b derives its
/// funding statuses from what is durable.
pub(super) fn reopen(f: &mut Fixture) {
    reopen_with(f, None);
}

/// How R8-01's lost record leaves the step's marker.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Marker {
    /// An older build's marker, which never had PA-2's guarantee: `Unknown`
    /// (OD-02).
    Unflagged,
    /// A flagged marker whose call released its funding signature (P2b's
    /// `funding_release` row, schema 7): a lock may have been sent with no
    /// store of it, so `MaybeSent` for good (E0-04 §2a.7, P2b r6 R6-01).
    Released,
    /// A flagged marker with no release, as a kill before the call's first
    /// signature leaves it: nothing was signed, so `NotSent` (C5).
    Unreleased,
}

/// Sol r8 R8-01's state: a funding call signed its lock, the row's outpoint
/// record failed, the process restarted, and the step's `signed_lock`
/// record is gone. PA-2 now stops the process before a lock is sent
/// without its record, so the journal is rewritten to what `marker` says
/// is left.
pub(super) fn unlanded(f: &mut Fixture, marker: Marker) -> (i64, OutPoint) {
    let (_, draft, original) = lock_record_failed(f);
    let conn = rusqlite::Connection::open(f.session.data_dir().join("dispatch.sqlite")).unwrap();
    conn.execute_batch("DELETE FROM signed_lock;").unwrap();
    match marker {
        Marker::Unflagged => conn
            .execute_batch("UPDATE funding_marker SET durable_record = 0;")
            .unwrap(),
        Marker::Released => {}
        Marker::Unreleased => conn.execute_batch("DELETE FROM funding_release;").unwrap(),
    }
    drop(conn);
    reopen(f);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    let status = if marker == Marker::Unreleased {
        DispatchState::NotSent
    } else {
        DispatchState::MaybeSent
    };
    assert_eq!(steps.len(), 1);
    assert_eq!(
        (
            steps[0].draft,
            steps[0].status,
            steps[0].signed.len(),
            steps[0].durable_record
        ),
        (draft, Some(status), 0, marker != Marker::Unflagged)
    );
    (draft, original)
}

pub(super) fn start(f: &Fixture, lib: &Arc<Fake>) -> Result<String, RegistrationError> {
    f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(lib),
        request("replace2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ))
}

/// R8-01a (I1, I4, I6): a marked step whose signed record was lost is
/// `Unknown`, never "no funding": its row is `Awaiting` (committed, waiting
/// for its funding), so discard is refused, a new registration waits, and
/// the row's own run neither funds again nor takes another lock.
#[test]
fn r8_01a_an_unknown_step_blocks_discard_a_second_start_and_a_second_build() {
    let mut f = fixture(false);
    let (draft, _) = unlanded(&mut f, Marker::Unflagged);
    // The restart lost the unpersisted lock too.
    let lib = f.fake();
    let status = f.status(&lib, draft);
    assert!(status.funds_committed);
    assert_eq!(status.waiting, Some(RegistrationWait::FundingUnknown));
    let discard = f.discard(&lib, draft);
    assert_eq!(discard.unwrap_err().code(), "invalid_argument");
    assert!(matches!(
        start(&f, &lib),
        Err(RegistrationError::InProgress { draft: d }) if d == draft.to_string()
    ));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(lib.counts().0, 0, "the row funded again");
    let row = f.row(draft);
    assert_eq!(
        (row.asset_lock_outpoint, row.error.as_deref()),
        (None, Some("platform.broadcast_unknown"))
    );
    // Failed or not, it still waits for its funding.
    let status = f.status(&lib, draft);
    assert_eq!(status.waiting, Some(RegistrationWait::FundingUnknown));
    assert_eq!(f.rows().len(), 1);
}

/// R8-01b (I5): while the step is `Unknown`, every lock it may have built
/// (its index, or a lost one) is `Contested`: "Finish transfers" gives none
/// to the row or to a recovery row, and counts the step and each lock.
#[test]
fn r8_01b_an_unknown_step_contests_the_locks_finish_could_substitute() {
    let mut f = fixture(false);
    let (draft, original) = unlanded(&mut f, Marker::Unflagged);
    let lib = f.fake();
    let other = OutPoint::new(Txid::from_byte_array([99; 32]), 0);
    lib.w()
        .locks
        .push(registration_lock(other, 0, LockStatus::RecoveredFromChain));
    let first = finish_once(&f, &lib);
    assert_eq!(first.still_pending, 2, "{first:?}");
    assert!(held_by(&f, other).is_empty());
    lib.w().locks.push(registration_lock(
        original,
        0,
        LockStatus::RecoveredFromChain,
    ));
    let second = finish_once(&f, &lib);
    assert_eq!(second.still_pending, 3, "{second:?}");
    assert!(held_by(&f, original).is_empty());
    assert_eq!(flow::outpoint_of(&f.row(draft)), None);
    assert_eq!(f.rows().len(), 1, "a recovery row for a contested lock");
}

/// PA-2 (§9, §13): P2b reads a flagged marked step with no release, no
/// signed record and no running call `NotSent` (a kill before the call's
/// first signature), so the step is not `Unknown`: it is cancelled and the row closes `Cancelled(funding_cancelled)` (C5,
/// Q14). A lock at its index back from the chain is `Free`, and the closed
/// row takes none (D15): it goes to a recovery row.
#[test]
fn r8_01_pa2_resolves_an_unknown_step() {
    let mut f = fixture(false);
    let (draft, original) = unlanded(&mut f, Marker::Unreleased);
    assert_eq!(
        f.session
            .leases
            .funding_status(f.wallet, &flow::funding_step(draft)),
        Some(DispatchState::NotSent)
    );
    let lib = f.fake();
    lib.w().locks.push(registration_lock(
        original,
        0,
        LockStatus::RecoveredFromChain,
    ));
    finish_once(&f, &lib);
    assert_eq!(
        f.row(draft).error.as_deref(),
        Some(crate::platform::registration_ledger::funding_cancelled())
    );
    let holders = held_by(&f, original);
    assert_eq!(holders.len(), 1, "{:?}", f.rows());
    assert_ne!(holders, [draft]);
    assert_eq!(lib.counts().0, 0);
}

/// A row an earlier build gave a substitute while its own signed lock was
/// away, whose identity (from the substitute) exists and whose name step
/// failed for good. Returns it and its own lock, back from the chain.
fn closed_signer(f: &Fixture, lib: &Fake) -> (i64, OutPoint) {
    let (draft, original) = signed_then_rejected(f, lib, core_row(PLAIN));
    let other = OutPoint::new(Txid::from_byte_array([99; 32]), 0);
    let moved = f
        .appdb()
        .update_registration(
            &f.wallet.to_string(),
            draft,
            f.row(draft).owner,
            Phase::KeysPrepared.as_str(),
            true,
            &RegistrationChange {
                phase: Some(Phase::NameRequested.as_str().into()),
                outpoint: Some(flow::canonical(other)),
                identity: Some(flow::b58(&flow::identity_for(other))),
                failure: Some(Some(("name.taken".into(), false))),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(moved, dw_appdb::RegistrationUpdate::Applied);
    lib.w().locks.push(registration_lock(
        original,
        0,
        LockStatus::RecoveredFromChain,
    ));
    (draft, original)
}

/// R8-02 (I2, I4): the signer of a lock that is back is `Closed`: it can no
/// longer take it, it stays committed (discard refused), and the lock goes
/// to neither it nor a row waiting for one.
#[test]
fn r8_02_a_closed_signer_is_committed_and_keeps_its_lock_from_other_rows() {
    let f = fixture(false);
    let lib = f.fake();
    let (draft, original) = closed_signer(&f, &lib);
    let waiting = f.insert(prepared_row(5));
    assert!(f.status(&lib, draft).funds_committed);
    let discard = f.discard(&lib, draft);
    assert_eq!(discard.unwrap_err().code(), "invalid_argument");
    finish_once(&f, &lib);
    let holders = held_by(&f, original);
    assert!(
        !holders.contains(&draft) && !holders.contains(&waiting),
        "{holders:?}"
    );
    assert_eq!(phase(&f.row(waiting)), Phase::KeysPrepared);
    assert_ne!(flow::outpoint_of(&f.row(draft)), Some(original));
}

/// R8-02, Q1: "Finish transfers" gives the closed signer's lock a recovery
/// row, which completes; the signer is then `Settled` and may be discarded.
#[test]
fn r8_02_finish_recovers_a_closed_signers_lock() {
    let f = fixture(false);
    let lib = f.fake();
    let (draft, original) = closed_signer(&f, &lib);
    finish_once(&f, &lib);
    let holders = held_by(&f, original);
    assert_eq!(holders.len(), 1, "{:?}", f.rows());
    assert_eq!(f.row(holders[0]).label, "");
    finish_once(&f, &lib);
    assert_eq!(phase(&f.row(holders[0])), Phase::Done);
    assert!(f.discard(&lib, draft).is_ok());
}

/// R8-03 (§5.2): the held path reports the same `pending` as the other: a
/// signed lock away under the sync hold is one.
#[test]
fn r8_03_a_signed_lock_away_is_pending_under_the_sync_hold() {
    let f = fixture(false);
    let lib = f.fake();
    signed_then_rejected(&f, &lib, core_row(PLAIN));
    lib.ready.store(false, Ordering::SeqCst);
    let report = finish_once(&f, &lib);
    assert_eq!(report.still_pending, 1, "{report:?}");
}

/// F6, Q5, Q7: a row deleted under its outstanding step (a restore that
/// lacks it) leaves an orphan step: `registrations()` lists it read-only,
/// and a new start names it. A restored copy whose funding key is exactly
/// the marker's is its heir: it waits for the step's lock, and takes it
/// when it is back.
#[test]
fn an_orphan_step_is_listed_until_its_exact_heir_takes_its_lock() {
    let f = fixture(false);
    let lib = f.fake();
    let (draft, original) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let row = f.row(draft);
    let index = row.identity_index.unwrap();
    assert!(
        f.appdb()
            .delete_registration(&f.wallet.to_string(), draft, row.owner, &row.phase, None)
            .unwrap()
    );
    let step = flow::funding_step(draft);
    let orphan = f
        .statuses(&lib)
        .into_iter()
        .find(|s| s.draft == step)
        .expect("the orphan step is listed");
    assert!(orphan.funds_committed);
    assert_eq!(orphan.waiting, Some(RegistrationWait::ChainLock));
    assert_eq!(orphan.txid, Some(original.txid.to_string()));
    assert!(matches!(
        start(&f, &lib),
        Err(RegistrationError::InProgress { draft: d }) if d == step
    ));
    let resumed = f.engine.block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        &step,
        None,
    ));
    assert_eq!(resumed.unwrap_err().code(), "invalid_argument");

    // The restored copy: the same funding key.
    let copy = f.insert(prepared_row(index));
    assert!(f.statuses(&lib).iter().all(|s| s.draft != step));
    assert_eq!(
        f.status(&lib, copy).waiting,
        Some(RegistrationWait::ChainLock)
    );
    assert_eq!(
        f.discard(&lib, copy).unwrap_err().code(),
        "invalid_argument"
    );
    f.drive(&lib, copy, Some(f.lease()), false);
    assert_eq!(lib.counts().0, 1, "the heir funded again");
    lib.w()
        .locks
        .push(registration_lock(original, index, LockStatus::Built));
    assert_eq!(tap(&f, original, index), Some(copy));
    resume(
        &f,
        &lib,
        copy,
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    );
    f.settle(copy);
    assert_eq!(phase(&f.row(copy)), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
}

/// Q5: a copy at the orphan's index whose funding key differs (here the
/// account) is no heir: the orphan stays listed, and the copy waits for
/// "Finish transfers" rather than fund the orphan's slot (§6 `index_owner`'s
/// step arm).
#[test]
fn a_copy_with_another_funding_key_does_not_inherit_an_orphan_step() {
    let f = fixture(false);
    let lib = f.fake();
    let (draft, index, sighash) = (77, 3, [0x55; 32]);
    let step = flow::funding_step(draft);
    let lease = f.lease();
    // P2b's funding gate reads the tracked rows, which load reads in the
    // background: read them first.
    assert!(
        f.engine
            .block_on(f.session.leases.refresh_funding(f.wallet))
    );
    f.engine
        .block_on(lease.clone().scope_step(Some(step.clone()), async {
            let key = FundingKey {
                funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
                identity_index: index,
                account: 1,
            };
            // A marker no draft key of this build's makes (another account):
            // under P2b's permit, past its draft-key check.
            let fence = f.session.coin_fence(f.wallet).await.unwrap();
            let quote = crate::platform::lease::FundingQuote {
                amount: flow::LOCK_DUFFS,
                fee_bound_worst: flow::fee_bound_worst(2),
            };
            let permit = lease
                .call_permit_step_fenced(
                    step.clone(),
                    Some(quote.into()),
                    crate::platform::lease::CallStep::Funding {
                        step: step.clone(),
                        key,
                    },
                    Some(fence),
                )
                .await
                .unwrap();
            permit.funding_signer().unwrap();
            permit.note_signed(sighash);
            f.session.leases.record_signed_locks(
                f.wallet,
                &[StoredLock {
                    txid: Txid::from_byte_array(sighash),
                    inputs: vec![OutPoint::new(Txid::from_byte_array([0x77; 32]), index)],
                    sighashes: Some(vec![sighash]),
                    built: true,
                }],
            );
            permit.finish(false);
        }));
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.draft, s.key.account, s.signed.len()))
            .collect::<Vec<_>>(),
        [(draft, 1, 1)]
    );
    let copy = f.insert(prepared_row(index));
    assert!(f.statuses(&lib).iter().any(|s| s.draft == step));
    f.drive(&lib, copy, Some(f.lease()), false);
    let row = f.row(copy);
    assert_eq!((row.asset_lock_outpoint, row.error), (None, None));
    assert_eq!(row.identity_index, Some(index));
    assert_eq!(lib.counts().0, 0, "the copy funded the orphan's slot");
    assert_eq!(
        f.status(&lib, copy).waiting,
        Some(RegistrationWait::ChainLock)
    );
}

/// I3: an `Ok` reporting a lock the step did not sign is not recorded. The
/// step's own lock, tracked, is.
#[test]
fn an_ok_that_is_not_the_steps_lock_records_the_steps_own() {
    let f = fixture(false);
    let fake = f.fake();
    fake.w().skip_tap = true;
    let other = OutPoint::new(Txid::from_byte_array([0x66; 32]), 0);
    let lib = Hooked {
        ok_returns: Some(other),
        ..Hooked::new(&fake)
    };
    let draft = f.insert(core_row(PLAIN));
    run_once(&f, &lib, draft).unwrap();
    let own = fake.w().locks[0].outpoint;
    assert_ne!(own, other);
    assert_eq!(flow::outpoint_of(&f.row(draft)), Some(own));
}

/// I3: with its own lock away, an `Ok` reporting another lock leaves the
/// row waiting for its own (`broadcast_unknown`), holding nothing.
#[test]
fn an_ok_that_is_not_the_steps_lock_is_never_recorded() {
    let f = fixture(false);
    let fake = f.fake();
    fake.w().skip_tap = true;
    let other = OutPoint::new(Txid::from_byte_array([0x66; 32]), 0);
    let lib = Hooked {
        ok_returns: Some(other),
        own_lock_away: true,
        ..Hooked::new(&fake)
    };
    let draft = f.insert(core_row(PLAIN));
    run_once(&f, &lib, draft).unwrap();
    let row = f.row(draft);
    assert_eq!(
        (row.asset_lock_outpoint, row.error.as_deref()),
        (None, Some("platform.broadcast_unknown"))
    );
    assert_eq!(
        f.status(&fake, draft).waiting,
        Some(RegistrationWait::ChainLock)
    );
}

/// Q4, C1 (`settled`'s in-place arm): a `Done` row reads `funds_committed` and may be discarded; its
/// step, every lock of it consumed, leaves no orphan behind.
#[test]
fn a_done_row_is_discarded_without_an_orphan() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert!(f.status(&lib, draft).funds_committed);
    f.discard(&lib, draft).unwrap();
    assert!(f.statuses(&lib).is_empty());
    // §11.2 item 9: nothing is pending, and a new start is allowed.
    assert_eq!(finish_once(&f, &lib).still_pending, 0);
    assert!(start(&f, &lib).is_ok());
}
