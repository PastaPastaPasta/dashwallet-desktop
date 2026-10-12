//! DP1-02's ledger by its tables (design rev2: §3.1 claims, §3.2
//! predicates, §3.3 standing, §3.4 owner, §5.1 `may_take`, §5.2
//! `reconcile`, §5.3 `pending`, §5.4 gates), on explicit evidence.

use platform_wallet::AssetLockFundingType;

use super::*;
use crate::platform::DispatchState;
use crate::platform::lease::mode_b::FundingStepView;
use crate::platform::registration_ledger::{
    IndexOwner, Ledger, Owner, Reconcile, Standing, Taker, funded_elsewhere, funding_cancelled,
};

fn txid(b: u8) -> Txid {
    Txid::from_byte_array([b; 32])
}

fn op(b: u8) -> OutPoint {
    OutPoint::new(txid(b), 0)
}

/// A readable Core-balance row `id` at `index`, `KeysPrepared`, nothing
/// funded.
fn row(id: i64, index: u32) -> RegistrationRow {
    RegistrationRow {
        id,
        identity_index: Some(index),
        identity: None,
        label: "alice".into(),
        temp_label: None,
        funding: Funding::CoreBalance {
            lock_duffs: flow::LOCK_DUFFS,
        }
        .encode(),
        initial_profile: None,
        asset_lock_outpoint: None,
        phase: Phase::KeysPrepared.as_str().into(),
        error: None,
        retryable: false,
        name_error: None,
        profile_error: None,
        owner: 0,
        created_at: 0,
        updated_at: 0,
    }
}

/// `row` holding `outpoint` at `phase`.
fn holding(mut r: RegistrationRow, outpoint: OutPoint, phase: Phase) -> RegistrationRow {
    r.asset_lock_outpoint = Some(outpoint.to_string());
    r.phase = phase.as_str().into();
    r
}

/// `row` as a newer build wrote it: this build cannot read it.
fn unreadable(mut r: RegistrationRow) -> RegistrationRow {
    r.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":3000000}"#.into();
    r
}

fn key(index: u32) -> FundingKey {
    FundingKey {
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index: index,
        account: 0,
    }
}

fn step(draft: i64, index: u32, status: Option<DispatchState>, signed: &[Txid]) -> FundingStepView {
    FundingStepView {
        draft,
        key: key(index),
        status,
        signed: signed.to_vec(),
        dead: Vec::new(),
        durable_record: false,
        recording: false,
    }
}

/// The test constructor of P2b's view (PA-1, PA-3, OD-02): a step whose
/// resolved attempts left `dead`, with its schema-6 flag.
fn tombstoned(
    draft: i64,
    index: u32,
    status: Option<DispatchState>,
    signed: &[Txid],
    dead: &[Txid],
    durable_record: bool,
) -> FundingStepView {
    FundingStepView {
        dead: dead.to_vec(),
        durable_record,
        recording: false,
        ..step(draft, index, status, signed)
    }
}

/// A step Repair resolved: its self-spend voided `t` (a tombstone), the
/// marker flagged and standing: P2b reads it `NotSent` (C4, Q13).
fn voided_step(draft: i64, index: u32, t: Txid) -> FundingStepView {
    tombstoned(draft, index, Some(DispatchState::NotSent), &[], &[t], true)
}

/// A step that may have signed and has no signed record (§3.1 `Unknown`).
fn unknown(draft: i64, index: u32) -> FundingStepView {
    step(draft, index, Some(DispatchState::MaybeSent), &[])
}

/// A step that signed `t` (§3.1 `Exact`).
fn exact(draft: i64, index: u32, t: Txid) -> FundingStepView {
    step(draft, index, Some(DispatchState::WillBeSent), &[t])
}

fn lock(b: u8, index: u32, status: LockStatus) -> LockInfo {
    LockInfo {
        outpoint: op(b),
        registration: true,
        identity_index: index,
        account: 0,
        amount: flow::LOCK_DUFFS,
        status,
    }
}

fn ledger(locks: Vec<LockInfo>, rows: Vec<RegistrationRow>, steps: Vec<FundingStepView>) -> Ledger {
    Ledger::new(locks, Vec::new(), rows, steps, Vec::new())
}

/// §3.3, line by line (first match wins).
#[test]
fn standing_rows_1_to_9() {
    // 1. Unreadable, committed by its outpoint only.
    let l = ledger(vec![], vec![unreadable(row(1, 0))], vec![]);
    assert_eq!(
        l.standing(&unreadable(row(1, 0))),
        Standing::Unreadable { committed: false }
    );
    let r = holding(unreadable(row(1, 0)), op(1), Phase::FundingSent);
    let l = ledger(vec![], vec![r.clone()], vec![]);
    assert_eq!(l.standing(&r), Standing::Unreadable { committed: true });

    // 2. Not a Core-balance funding: nothing of the user's to fund.
    let mut existing = row(1, 0);
    existing.funding = Funding::ExistingIdentity.encode();
    let l = ledger(vec![], vec![existing.clone()], vec![exact(1, 0, txid(9))]);
    assert_eq!(l.standing(&existing), Standing::Unfunded);

    // 3. Holds a lock that is not consumed.
    let r = holding(row(1, 0), op(9), Phase::FundingSent);
    let l = ledger(
        vec![lock(9, 0, LockStatus::Built)],
        vec![r.clone()],
        vec![exact(1, 0, txid(9))],
    );
    assert_eq!(l.standing(&r), Standing::Funded(op(9)));

    // 4. An outstanding step, open: `Exact` (lock away) and `Unknown`.
    let r = row(1, 0);
    for s in [exact(1, 0, txid(9)), unknown(1, 0)] {
        let l = ledger(vec![], vec![r.clone()], vec![s.clone()]);
        assert_eq!(l.standing(&r), Standing::Awaiting, "{s:?}");
    }

    // 5. An outstanding step, not open: the row holds a substitute whose
    //    identity exists (an earlier build), its own lock is elsewhere.
    let mut r = holding(row(1, 0), op(8), Phase::Done);
    r.identity = Some(flow::b58(&flow::identity_for(op(8))));
    let l = ledger(
        vec![lock(8, 0, LockStatus::Consumed)],
        vec![r.clone()],
        vec![exact(1, 0, txid(9))],
    );
    assert_eq!(l.standing(&r), Standing::Closed);

    // 6. Its identity exists and nothing is outstanding.
    let l = ledger(
        vec![lock(9, 0, LockStatus::Consumed)],
        vec![r_with(op(9))],
        vec![exact(1, 0, txid(9))],
    );
    assert_eq!(l.standing(&r_with(op(9))), Standing::Settled);

    // 6b. Its live lock is consumed (no identity recorded yet).
    let r = holding(row(1, 0), op(9), Phase::FundingSent);
    let l = ledger(
        vec![lock(9, 0, LockStatus::Consumed)],
        vec![r.clone()],
        vec![],
    );
    assert_eq!(l.standing(&r), Standing::Settled);

    // 7. Its signer's lock is held by another row (a restored copy).
    let a = row(1, 0);
    let b = holding(row(2, 0), op(9), Phase::FundingSent);
    let l = ledger(
        vec![],
        vec![a.clone(), b.clone()],
        vec![exact(1, 0, txid(9))],
    );
    assert_eq!(l.standing(&a), Standing::Cancelled(funded_elsewhere()));
    assert_eq!(l.standing(&b), Standing::Funded(op(9)));

    // 8. Its step is cancelled (a standing marker whose claim is `None`),
    //    or it records a voided outpoint.
    let r = row(1, 0);
    let l = ledger(
        vec![],
        vec![r.clone()],
        vec![step(1, 0, Some(DispatchState::NotSent), &[txid(9)])],
    );
    assert_eq!(l.standing(&r), Standing::Cancelled(funding_cancelled()));
    let v = holding(row(1, 0), op(9), Phase::FundingSent);
    let l = ledger(vec![], vec![v.clone()], vec![voided_step(5, 3, txid(9))]);
    assert_eq!(l.standing(&v), Standing::Cancelled(funding_cancelled()));
    // The written code alone keeps it, whatever the evidence reads now.
    for code in [funded_elsewhere(), funding_cancelled()] {
        let mut failed = row(1, 0);
        failed.error = Some(code.into());
        let l = ledger(vec![], vec![failed.clone()], vec![]);
        assert_eq!(l.standing(&failed), Standing::Cancelled(code));
    }
    // Never while calling (rows 7–8 read ¬calling(r)).
    let l = Ledger::new(
        vec![],
        vec![],
        vec![r.clone()],
        vec![step(1, 0, Some(DispatchState::NotSent), &[])],
        vec![1],
    );
    assert_eq!(l.standing(&r), Standing::Unfunded);
    assert!(l.committed(&r));

    // 9. Otherwise: never funded, or its step left the view.
    let l = ledger(vec![], vec![r.clone()], vec![]);
    assert_eq!(l.standing(&r), Standing::Unfunded);
}

/// §3.1 with Q13: a tombstoned step's claim is P2b's status, never a local
/// override. P2b reads a flagged one resolved (C4).
#[test]
fn claims_read_a_tombstoned_step_by_p2bs_status() {
    let r = row(1, 0);
    // P2b's C4 reading: resolved, so `None`.
    for status in [Some(DispatchState::NotSent), None] {
        let l = ledger(
            vec![lock(5, 0, LockStatus::Broadcast)],
            vec![r.clone()],
            vec![tombstoned(1, 0, status, &[], &[txid(9)], true)],
        );
        assert_eq!(l.pending(), 1, "{status:?}: U is Free, not Contested");
        assert_eq!(
            l.owner(&lock(5, 0, LockStatus::Broadcast)),
            Some(Owner::Free)
        );
        assert_eq!(l.standing(&r), Standing::Cancelled(funding_cancelled()));
    }
    // A flagged step P2b still reads outstanding is `Unknown`: the ledger
    // never overrides it.
    for status in [Some(DispatchState::MaybeSent), Some(DispatchState::Sent)] {
        let l = ledger(
            vec![lock(5, 0, LockStatus::Broadcast)],
            vec![r.clone()],
            vec![tombstoned(1, 0, status, &[], &[txid(9)], true)],
        );
        assert_eq!(
            l.owner(&lock(5, 0, LockStatus::Broadcast)),
            Some(Owner::Contested),
            "{status:?}"
        );
        assert!(l.funding_unknown(&r), "{status:?}");
    }
    // Unflagged: `Unknown`, so U is `Contested` and the row `Awaiting`.
    let l = ledger(
        vec![lock(5, 0, LockStatus::Broadcast)],
        vec![r.clone()],
        vec![tombstoned(
            1,
            0,
            Some(DispatchState::MaybeSent),
            &[],
            &[txid(9)],
            false,
        )],
    );
    assert_eq!(
        l.owner(&lock(5, 0, LockStatus::Broadcast)),
        Some(Owner::Contested)
    );
    assert_eq!(l.standing(&r), Standing::Awaiting);
    // A live attempt beside tombstones is `Exact` by its signed txids.
    let l = ledger(
        vec![],
        vec![r.clone()],
        vec![tombstoned(
            1,
            0,
            Some(DispatchState::WillBeSent),
            &[txid(8)],
            &[txid(9)],
            true,
        )],
    );
    assert_eq!(l.standing(&r), Standing::Awaiting);
    assert!(!l.funding_unknown(&r));
}

/// A `Done` row holding the consumed lock `o` whose identity it records.
fn r_with(o: OutPoint) -> RegistrationRow {
    let mut r = holding(row(1, 0), o, Phase::Done);
    r.identity = Some(flow::b58(&flow::identity_for(o)));
    r
}

/// §3.4, line by line, and `None` for a held or consumed lock.
#[test]
fn owner_rows_1_to_5() {
    // 1. Signed by a step that is not outstanding (resolved `NotSent`),
    //    or a tombstone, even beside a live attempt of its step.
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![row(1, 1)],
        vec![step(1, 1, Some(DispatchState::NotSent), &[txid(9)])],
    );
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Built)), Some(Owner::Dead));
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![row(1, 1)],
        vec![tombstoned(
            1,
            1,
            Some(DispatchState::WillBeSent),
            &[txid(8)],
            &[txid(9)],
            true,
        )],
    );
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Built)), Some(Owner::Dead));
    // A held voided lock is no row's: `Dead` too.
    let v = holding(row(2, 4), op(9), Phase::FundingSent);
    let l = ledger(vec![], vec![v], vec![voided_step(1, 1, txid(9))]);
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Built)), Some(Owner::Dead));

    // 2. Signed by an outstanding step: its signer's while unconsumed;
    //    consumed signed value is §5.2a's.
    let l = ledger(vec![], vec![row(1, 1)], vec![exact(1, 1, txid(9))]);
    assert_eq!(
        l.owner(&lock(9, 1, LockStatus::Built)),
        Some(Owner::Step(1))
    );
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Consumed)), None);

    // 3. A row this build cannot read at its index (or any, index lost).
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![unreadable(row(1, 1))],
        vec![],
    );
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Built)), Some(Owner::Parked));
    let l = ledger(
        vec![lock(9, 0, LockStatus::Broadcast)],
        vec![unreadable(row(1, 4))],
        vec![],
    );
    assert_eq!(
        l.owner(&lock(9, 0, LockStatus::Broadcast)),
        Some(Owner::Parked)
    );

    // 4. An `Unknown` step that holds no lock: its index, or a lost one,
    //    unless the amount differs from the one its row locks (Q6).
    let l = ledger(vec![], vec![row(1, 1)], vec![unknown(1, 1)]);
    assert_eq!(
        l.owner(&lock(9, 1, LockStatus::Built)),
        Some(Owner::Contested)
    );
    assert_eq!(
        l.owner(&lock(9, 0, LockStatus::Broadcast)),
        Some(Owner::Contested)
    );
    assert_eq!(l.owner(&lock(9, 2, LockStatus::Built)), Some(Owner::Free));
    let mut other = lock(9, 1, LockStatus::Built);
    other.amount += 1;
    assert_eq!(l.owner(&other), Some(Owner::Free), "Q6");
    // No readable amount (an orphan step): not narrowed.
    let l = ledger(vec![], vec![], vec![unknown(1, 1)]);
    assert_eq!(l.owner(&other), Some(Owner::Contested));

    // 5. Otherwise free.
    let l = ledger(vec![], vec![row(1, 1)], vec![]);
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Built)), Some(Owner::Free));

    // A held lock, and a consumed lock no step signed, have no owner.
    let l = ledger(
        vec![],
        vec![holding(row(1, 1), op(9), Phase::FundingSent)],
        vec![],
    );
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Built)), None);
    let l = ledger(vec![], vec![row(1, 1)], vec![]);
    assert_eq!(l.owner(&lock(9, 1, LockStatus::Consumed)), None);
}

/// §5.1.
#[test]
fn may_take_matrix() {
    let built = lock(9, 1, LockStatus::Built);
    // Step(S): its row while open; a recovery row once that row is gone
    // or no longer open.
    let l = ledger(
        vec![],
        vec![row(1, 1), row(2, 1)],
        vec![exact(1, 1, txid(9))],
    );
    assert!(l.may_take(Taker::Row(1), &built));
    assert!(!l.may_take(Taker::Row(2), &built));
    assert!(!l.may_take(Taker::Recovery, &built));
    let closed = r_with(op(8));
    let l = ledger(
        vec![lock(8, 0, LockStatus::Consumed)],
        vec![closed],
        vec![exact(1, 1, txid(9))],
    );
    assert!(!l.may_take(Taker::Row(1), &built));
    assert!(l.may_take(Taker::Recovery, &built), "row_of not open");
    let l = ledger(vec![], vec![row(2, 5)], vec![exact(1, 1, txid(9))]);
    assert!(!l.may_take(Taker::Row(2), &built));
    assert!(l.may_take(Taker::Recovery, &built), "row_of none");

    // Step(S) while its open row calls (OD-05): the row, never Recovery.
    let l = Ledger::new(
        vec![],
        vec![],
        vec![row(1, 1)],
        vec![exact(1, 1, txid(9))],
        vec![1],
    );
    assert!(l.may_take(Taker::Row(1), &built), "calling or not");
    assert!(!l.may_take(Taker::Recovery, &built));
    // A holder beats the signer: the restored copy holding T1 is row_of.
    let holder = holding(row(2, 1), op(9), Phase::FundingSent);
    let l = ledger(vec![], vec![row(1, 1), holder], vec![exact(1, 1, txid(9))]);
    assert_eq!(l.owner(&built), None, "held");
    assert_eq!(
        l.standing(&row(1, 1)),
        Standing::Cancelled(funded_elsewhere())
    );

    // Free: an Unfunded open row that isn't calling, or a recovery row.
    let l = ledger(vec![], vec![row(1, 1), row(2, 1)], vec![unknown(2, 3)]);
    let free = lock(9, 1, LockStatus::Built);
    assert_eq!(l.owner(&free), Some(Owner::Free));
    assert!(l.may_take(Taker::Row(1), &free));
    assert!(!l.may_take(Taker::Row(2), &free), "an Awaiting row");
    assert!(l.may_take(Taker::Recovery, &free));
    let l = Ledger::new(vec![], vec![], vec![row(1, 1)], vec![], vec![1]);
    assert!(!l.may_take(Taker::Row(1), &free), "OD-05: a calling row");
    assert!(l.may_take(Taker::Recovery, &free));
    // A cancelled row never takes a `Free` lock.
    let mut cancelled = row(1, 1);
    cancelled.error = Some(funding_cancelled().into());
    let l = ledger(vec![], vec![cancelled], vec![]);
    assert!(!l.may_take(Taker::Row(1), &free));

    // Parked, Contested, Dead: nobody.
    let cases = [
        ledger(vec![], vec![row(1, 1), unreadable(row(2, 1))], vec![]),
        ledger(vec![], vec![row(1, 1), row(2, 1)], vec![unknown(2, 1)]),
        ledger(
            vec![],
            vec![row(1, 1)],
            vec![step(2, 1, Some(DispatchState::NotSent), &[txid(9)])],
        ),
    ];
    for l in cases {
        assert!(!l.may_take(Taker::Row(1), &built));
        assert!(!l.may_take(Taker::Recovery, &built));
    }
}

/// §5.2: outstanding steps no row holds a lock of, plus `Parked`,
/// `Contested` and `Free` locks.
#[test]
fn pending_formula() {
    // An Unknown step and a Contested lock that may be its count as two.
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![row(1, 1)],
        vec![unknown(1, 1)],
    );
    assert_eq!(
        l.owner(&lock(9, 1, LockStatus::Built)),
        Some(Owner::Contested)
    );
    assert_eq!(l.pending(), 2);
    // A Step lock counts once, through its step.
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![row(1, 1)],
        vec![exact(1, 1, txid(9))],
    );
    assert_eq!(l.pending(), 1);
    // Held: nothing.
    let r = holding(row(1, 1), op(9), Phase::FundingSent);
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![r],
        vec![exact(1, 1, txid(9))],
    );
    assert_eq!(l.pending(), 0);
    // Dead locks are not money; Parked and Free are.
    let l = ledger(
        vec![
            lock(9, 1, LockStatus::Built),
            lock(7, 2, LockStatus::Built),
            lock(6, 3, LockStatus::Built),
        ],
        vec![unreadable(row(2, 2))],
        vec![step(1, 1, Some(DispatchState::NotSent), &[txid(9)])],
    );
    assert_eq!(l.pending(), 2);
}

/// §3.2's heir: an exact funding-key match only (Q5), oldest open row
/// first, one step each; otherwise an orphan.
#[test]
fn an_orphan_step_has_an_heir_only_on_an_exact_key() {
    let s = exact(5, 1, txid(9));
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![row(7, 1)],
        vec![s.clone()],
    );
    assert_eq!(l.orphans(), vec![]);
    assert_eq!(l.standing(&row(7, 1)), Standing::Awaiting);
    assert_eq!(
        l.owner(&lock(9, 1, LockStatus::Built)),
        Some(Owner::Step(5))
    );
    assert!(l.may_take(Taker::Row(7), &lock(9, 1, LockStatus::Built)));

    // Another index, or another account: no heir.
    let mut foreign = s.clone();
    foreign.key.account = 1;
    for (rows, steps) in [
        (vec![row(7, 2)], vec![s.clone()]),
        (vec![row(7, 1)], vec![foreign]),
    ] {
        let l = ledger(vec![], rows.clone(), steps);
        assert_eq!(l.orphans(), vec![(5, false)]);
        assert_eq!(l.standing(&rows[0]), Standing::Unfunded);
        // The step arm: another draft's outstanding step at the index.
        if rows[0].identity_index == Some(1) {
            assert_eq!(l.index_owner(&rows[0], 1), IndexOwner::Unknown);
        }
    }
    // I6: an orphan blocks a new start, named by its step (Q7).
    let l = ledger(vec![], vec![], vec![s.clone()]);
    assert_eq!(l.blocking(), Some(flow::funding_step(5)));

    // A row with an outstanding step of its own inherits nothing.
    let l = ledger(vec![], vec![row(7, 1)], vec![s, unknown(7, 1)]);
    assert_eq!(l.orphans(), vec![(5, false)]);

    // `settled`'s in-place arm (C1): an orphan whose signed lock is
    // consumed, with no row to finish it, holds nothing more; its slot is
    // `Taken`, not `Unknown`.
    let l = ledger(
        vec![lock(9, 1, LockStatus::Consumed)],
        vec![],
        vec![exact(5, 1, txid(9))],
    );
    assert_eq!(l.orphans(), vec![]);
    assert_eq!(l.blocking(), None);
    assert_eq!(l.pending(), 0);
    let l = ledger(
        vec![lock(9, 1, LockStatus::Consumed)],
        vec![row(7, 2)],
        vec![exact(5, 1, txid(9))],
    );
    assert_eq!(l.index_owner(&row(7, 1), 1), IndexOwner::Taken);
    // Its heir (open) still has to receive the identity: outstanding.
    let l = ledger(
        vec![lock(9, 1, LockStatus::Consumed)],
        vec![row(7, 1)],
        vec![exact(5, 1, txid(9))],
    );
    assert_eq!(l.standing(&row(7, 1)), Standing::Awaiting);
    assert!(l.awaits_discovery(&row(7, 1)));
    assert_eq!(l.pending(), 1);
}

/// `committed` (§3.3), the gates (§5.3) and `next_index` (§6).
#[test]
fn gates_by_standing() {
    // Unfunded: committed only while its call runs.
    let r = row(1, 1);
    let l = Ledger::new(vec![], vec![], vec![r.clone()], vec![], vec![1]);
    assert!(l.committed(&r));
    assert!(l.discard_refused(&r).is_some());
    let l = ledger(vec![], vec![r.clone()], vec![]);
    assert!(!l.committed(&r));
    assert_eq!(l.discard_refused(&r), None);

    // Awaiting: committed, in progress although failed for good, not
    // resumable.
    let mut failed = row(1, 1);
    failed.error = Some(flow::broadcast_unknown().into());
    let l = ledger(vec![], vec![failed.clone()], vec![unknown(1, 1)]);
    assert!(l.committed(&failed));
    assert!(l.in_progress(&failed));
    assert!(!l.resumable(&failed));
    assert!(l.discard_refused(&failed).is_some());
    assert!(l.funding_unknown(&failed));
    // Unfunded with `broadcast_unknown` (its step left the view): resumable.
    let l = ledger(vec![], vec![failed.clone()], vec![]);
    assert!(l.resumable(&failed));
    assert!(!l.in_progress(&failed));
    // Cancelled (its step's marker stands, resolved): discardable, not in
    // progress, never resumed, not `funds_committed`.
    let l = ledger(
        vec![],
        vec![failed.clone()],
        vec![step(1, 1, Some(DispatchState::NotSent), &[])],
    );
    assert_eq!(
        l.standing(&failed),
        Standing::Cancelled(funding_cancelled())
    );
    assert!(!l.resumable(&failed));
    assert!(!l.in_progress(&failed));
    assert!(!l.committed(&failed));
    assert_eq!(l.discard_refused(&failed), None);
    assert!(!l.funds_committed(&failed));
    assert_eq!(l.blocking(), None);

    // Settled (Done): discardable, not committed, yet its status still
    // reads `funds_committed` (Q4).
    let done = r_with(op(9));
    let l = ledger(
        vec![lock(9, 0, LockStatus::Consumed)],
        vec![done.clone()],
        vec![],
    );
    assert_eq!(l.standing(&done), Standing::Settled);
    assert!(!l.committed(&done));
    assert_eq!(l.discard_refused(&done), None);
    assert!(l.funds_committed(&done));
    assert!(!l.in_progress(&done));

    // next_index skips every step's key index: standing, settled or
    // cancelled (I12).
    let l = ledger(vec![], vec![row(1, 1)], vec![unknown(4, 9)]);
    assert_eq!(l.next_index(), 10);
    let l = ledger(
        vec![],
        vec![row(1, 1)],
        vec![step(4, 9, Some(DispatchState::NotSent), &[])],
    );
    assert_eq!(l.next_index(), 10);
    let l = ledger(
        vec![lock(9, 9, LockStatus::Consumed)],
        vec![row(1, 1)],
        vec![exact(4, 9, txid(9))],
    );
    assert_eq!(l.next_index(), 10);
    // index_owner's step arm: an outstanding step `Unknown`, else `Taken`.
    let l = ledger(vec![], vec![row(1, 9)], vec![unknown(4, 9)]);
    assert_eq!(l.index_owner(&row(1, 9), 9), IndexOwner::Unknown);
    let l = ledger(vec![], vec![row(1, 9)], vec![voided_step(4, 9, txid(7))]);
    assert_eq!(l.index_owner(&row(1, 9), 9), IndexOwner::Taken);
}

/// §3.2 `live`/`voided` and §5.2b: the exact tombstone match clears, and
/// closes a row with nothing outstanding.
#[test]
fn a_voided_outpoint_is_cleared_and_its_row_closed() {
    let v = holding(row(1, 1), op(9), Phase::FundingSent);
    let l = ledger(vec![], vec![v.clone()], vec![voided_step(1, 1, txid(9))]);
    assert_eq!(l.live(&v), None);
    assert!(l.open(&v));
    assert_eq!(l.standing(&v), Standing::Cancelled(funding_cancelled()));
    assert!(!l.committed(&v));
    assert_eq!(
        l.reconcile(),
        vec![Reconcile::Close {
            row: 1,
            clear: true,
            code: Some(funding_cancelled()),
        }]
    );
    // A live substitute T2 is never cleared because T1 was resolved.
    let t2 = holding(row(1, 1), op(8), Phase::FundingSent);
    let l = ledger(
        vec![lock(8, 1, LockStatus::Broadcast)],
        vec![t2.clone()],
        vec![voided_step(1, 1, txid(9))],
    );
    assert_eq!(l.standing(&t2), Standing::Funded(op(8)));
    assert_eq!(l.reconcile(), vec![]);
    // An `Awaiting` row beside its voided outpoint: `clear` only; it still
    // takes its own T1.
    let a = holding(row(1, 1), op(8), Phase::FundingSent);
    let l = ledger(
        vec![lock(9, 1, LockStatus::Built)],
        vec![a.clone()],
        vec![
            exact(1, 1, txid(9)),
            tombstoned(2, 5, Some(DispatchState::NotSent), &[], &[txid(8)], true),
        ],
    );
    assert_eq!(l.standing(&a), Standing::Awaiting);
    assert_eq!(
        l.reconcile(),
        vec![Reconcile::Close {
            row: 1,
            clear: true,
            code: None,
        }]
    );
    assert_eq!(l.own_lock(&a).map(|l| l.outpoint), Some(op(9)));
    // Idempotent: once cleared and failed with the code, nothing to write.
    let mut closed = row(1, 1);
    closed.error = Some(funding_cancelled().into());
    let l = ledger(vec![], vec![closed], vec![voided_step(1, 1, txid(9))]);
    assert_eq!(l.reconcile(), vec![]);
    // `funded_elsewhere` wins when both apply.
    let mut both = row(1, 1);
    both.error = Some(funding_cancelled().into());
    let b = holding(row(2, 1), op(9), Phase::FundingSent);
    let l = ledger(vec![], vec![both.clone(), b], vec![exact(1, 1, txid(9))]);
    assert_eq!(l.standing(&both), Standing::Cancelled(funded_elsewhere()));
    assert_eq!(
        l.reconcile(),
        vec![Reconcile::Close {
            row: 1,
            clear: false,
            code: Some(funded_elsewhere()),
        }]
    );
    // A calling row is never reconciled.
    let l = Ledger::new(
        vec![],
        vec![],
        vec![v],
        vec![voided_step(1, 1, txid(9))],
        vec![1],
    );
    assert_eq!(l.reconcile(), vec![]);
    // Review M1: a row this build cannot read is never written, even
    // beside the tombstone that voids its outpoint.
    let u = unreadable(holding(row(1, 1), op(9), Phase::FundingSent));
    let l = ledger(vec![], vec![u], vec![voided_step(1, 1, txid(9))]);
    assert_eq!(l.reconcile(), vec![]);
}

/// D15 (review m3): a row closed `funded_elsewhere` stays closed when the
/// holder that justified the close goes; its step is no longer the row's,
/// so the step's unconsumed lock goes to `Recovery`, never back to it.
#[test]
fn a_written_close_is_sticky() {
    let mut closed = row(1, 1);
    closed.error = Some(funded_elsewhere().into());
    let t = lock(9, 1, LockStatus::Broadcast);
    let l = ledger(
        vec![t.clone()],
        vec![closed.clone()],
        vec![exact(1, 1, txid(9))],
    );
    assert_eq!(l.standing(&closed), Standing::Cancelled(funded_elsewhere()));
    assert!(!l.committed(&closed));
    assert_eq!(l.reconcile(), vec![]);
    assert!(!l.may_take(Taker::Row(1), &t));
    assert!(l.may_take(Taker::Recovery, &t));
    assert_eq!(l.orphans(), vec![(1, false)]);
    // The same row, not closed, still takes its own step's lock.
    let open = row(1, 1);
    let l = ledger(
        vec![t.clone()],
        vec![open.clone()],
        vec![exact(1, 1, txid(9))],
    );
    assert_eq!(l.standing(&open), Standing::Awaiting);
    assert!(l.may_take(Taker::Row(1), &t));
    assert!(!l.may_take(Taker::Recovery, &t));
    // A closed row is no heir either: a recordless orphan's lock goes to
    // `Recovery`.
    let mut heir = row(2, 1);
    heir.error = Some(funding_cancelled().into());
    let l = ledger(
        vec![t.clone()],
        vec![heir.clone()],
        vec![exact(7, 1, txid(9))],
    );
    assert_eq!(l.standing(&heir), Standing::Cancelled(funding_cancelled()));
    assert!(!l.may_take(Taker::Row(2), &t));
    assert!(l.may_take(Taker::Recovery, &t));
    // The exclusion applies to the chosen candidate, a holder too, and
    // never falls through to the signer (Sol design r2): a closed row
    // holding T1 leaves the step with no row, although its open signer
    // stands.
    let mut held = holding(row(2, 1), op(9), Phase::FundingSent);
    held.error = Some(funded_elsewhere().into());
    let l = ledger(
        vec![t.clone()],
        vec![row(1, 1), held],
        vec![exact(1, 1, txid(9))],
    );
    assert_eq!(l.orphans(), vec![(1, false)]);
    // §3.3 row 7 reads the holder, which stays evidence: A is closed too.
    assert_eq!(
        l.standing(&row(1, 1)),
        Standing::Cancelled(funded_elsewhere())
    );
}

/// §5.2a: consumed signed value goes to its open row once discovery
/// reports the identity; never while undiscovered, never to a closed row.
#[test]
fn consumed_signed_value_attaches_only_once_discovered() {
    let a = row(1, 1);
    let id = flow::b58(&flow::identity_for(op(9)));
    let consumed = vec![lock(9, 1, LockStatus::Consumed)];
    let undiscovered = ledger(
        consumed.clone(),
        vec![a.clone()],
        vec![exact(1, 1, txid(9))],
    );
    assert_eq!(undiscovered.standing(&a), Standing::Awaiting);
    assert!(undiscovered.awaits_discovery(&a));
    assert_eq!(undiscovered.reconcile(), vec![]);
    assert_eq!(undiscovered.pending(), 1);
    assert_eq!(undiscovered.owner(&consumed[0]), None, "never Recovery");

    let l = Ledger::new(
        consumed.clone(),
        vec![(4, id.clone())],
        vec![a.clone()],
        vec![exact(1, 1, txid(9))],
        vec![],
    );
    assert_eq!(
        l.reconcile(),
        vec![Reconcile::Attach {
            row: 1,
            lock: op(9),
            identity: id.clone(),
            index: 4,
        }]
    );
    // Not while calling.
    let l = Ledger::new(
        consumed.clone(),
        vec![(4, id.clone())],
        vec![a.clone()],
        vec![exact(1, 1, txid(9))],
        vec![1],
    );
    assert_eq!(l.reconcile(), vec![]);
    // row_of not open: settles in place, no write.
    let mut other = holding(row(1, 1), op(7), Phase::Done);
    other.identity = Some(flow::b58(&flow::identity_for(op(7))));
    let l = Ledger::new(
        consumed,
        vec![(4, id)],
        vec![other.clone()],
        vec![exact(1, 1, txid(9))],
        vec![],
    );
    assert_eq!(l.reconcile(), vec![]);
    assert_eq!(l.pending(), 0);
    assert_eq!(l.standing(&other), Standing::Settled);
}

/// DEC-197: only a step that recorded no lock is wedged, so passed. A
/// flagged `MaybeSent` step with a tombstone (P2b reading a step Repair
/// resolved as unknown) is not, nor is one under a record write or a call.
#[test]
fn a_wedged_step_recorded_no_lock() {
    let mut awaiting = row(1, 1);
    awaiting.error = Some(flow::broadcast_unknown().into());
    let flagged = |dead: &[Txid]| tombstoned(1, 1, Some(DispatchState::MaybeSent), &[], dead, true);
    let l = ledger(vec![], vec![awaiting.clone()], vec![flagged(&[])]);
    assert_eq!(l.standing(&awaiting), Standing::Awaiting);
    assert_eq!(l.passed(&[1]), vec![1]);
    let recording = FundingStepView {
        recording: true,
        ..flagged(&[])
    };
    for s in [flagged(&[txid(9)]), recording] {
        let l = ledger(vec![], vec![awaiting.clone()], vec![s]);
        assert_eq!(l.passed(&[1]), Vec::<i64>::new());
    }
    let l = Ledger::new(vec![], vec![], vec![awaiting], vec![flagged(&[])], vec![1]);
    assert_eq!(l.passed(&[1]), Vec::<i64>::new());
}

/// DEC-198: `Recovery` takes a lock only wedged steps contest, each with a
/// known release, and a row never does. A contester in a call, under a
/// record write, with a tombstone, unflagged, or with no known release
/// keeps it `Contested` for everyone.
#[test]
fn recovery_takes_a_lock_only_released_wedged_steps_contest() {
    let mut awaiting = row(1, 1);
    awaiting.error = Some(flow::broadcast_unknown().into());
    let rebuilt = lock(9, 0, LockStatus::RecoveredFromChain);
    let flagged = tombstoned(1, 1, Some(DispatchState::MaybeSent), &[], &[], true);
    let read = |view: FundingStepView, calling: Vec<i64>, released: bool| {
        let rows = vec![awaiting.clone()];
        let mut l = Ledger::new(vec![rebuilt.clone()], vec![], rows, vec![view], calling);
        if released {
            l.set_released(vec![1]);
        }
        l
    };
    let l = read(flagged.clone(), vec![], true);
    assert_eq!(l.owner(&rebuilt), Some(Owner::Contested));
    assert!(l.may_take(Taker::Recovery, &rebuilt));
    assert!(!l.may_take(Taker::Row(1), &rebuilt));
    // A new row C waits for Finish, not the ChainLock (DEC-198 am.1).
    let c = row(2, 2);
    let mut l = Ledger::new(
        vec![rebuilt.clone()],
        vec![],
        vec![awaiting.clone(), c.clone()],
        vec![flagged.clone()],
        vec![],
    );
    assert!(l.unattributed(&c));
    assert_eq!(l.unattributed_wait(&c), RegistrationWait::ChainLock);
    l.set_released(vec![1]);
    assert_eq!(l.unattributed_wait(&c), RegistrationWait::NeedsFinish);
    let recording = FundingStepView {
        recording: true,
        ..flagged.clone()
    };
    let voided = tombstoned(1, 1, Some(DispatchState::MaybeSent), &[], &[txid(8)], true);
    for (view, calling, released) in [
        (flagged.clone(), vec![1], true),
        (recording, vec![], true),
        (voided, vec![], true),
        (unknown(1, 1), vec![], true),
        (flagged, vec![], false),
    ] {
        let l = read(view.clone(), calling.clone(), released);
        assert_eq!(l.owner(&rebuilt), Some(Owner::Contested), "{view:?}");
        assert!(
            !l.may_take(Taker::Recovery, &rebuilt),
            "{view:?} {calling:?} {released}"
        );
    }
}
