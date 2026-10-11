//! DP1-02 rev2 §11.2: OD-03 to OD-06, C1, C4, I12 and D15 on the real lease
//! table, journal and SQLite with the Fake library. P2b's rev2 asks
//! (tombstones, the schema-6 flag, `StepCancelled`) run on P2b's own path:
//! [`repair`] resolves a step as Repair does. A resolved step's view is set
//! through [`Views`] only where the ledger's rules are the subject.

use super::r8::{reopen, start, unlanded};
use super::*;
use crate::platform::lease::mode_b::FundingStepView;
use crate::platform::registration_ledger::test_views::Views;
use crate::platform::registration_ledger::{
    IndexOwner, Ledger, Reconcile, Standing, funded_elsewhere, funding_cancelled,
};

fn ledger(f: &Fixture, lib: &Fake) -> Ledger {
    f.engine
        .block_on(Ledger::load(&f.session, f.wallet, lib))
        .unwrap()
}

/// The library's broadcast of Repair's self-spend, faked: it keeps the
/// transaction (P2b's `SelfSpendSeam`).
#[derive(Default)]
struct SelfSpendSeam(std::sync::Mutex<Option<dashcore::Transaction>>);

impl crate::send::TestBroadcaster for SelfSpendSeam {
    fn before_entry(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(std::future::ready(()))
    }

    fn broadcast(
        &self,
        tx: &dashcore::Transaction,
        _first: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Txid, platform_wallet::PlatformWalletError>> + Send>>
    {
        *self.0.lock().unwrap() = Some(tx.clone());
        let txid = tx.txid();
        Box::pin(async move { Ok(txid) })
    }
}

/// A real coin of the wallet that the funding's lock spends, so Repair's
/// self-spend can take it.
fn lock_on_a_coin(f: &Fixture, lib: &Fake) {
    let (_, coin) = f.coin();
    lib.w().lock_input = Some(coin);
}

/// P2b's Repair of `draft`'s stuck funding step (E0-04 §6.5, PA-3): the
/// self-spend of every funding coin, the step's lock's input among them,
/// sent and then recorded ChainLocked. P2b reads the step `NotSent`, its
/// signed locks tombstoned.
fn repair(f: &Fixture, draft: i64) {
    use key_wallet::account::{AccountType, StandardAccountType};
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    use key_wallet::managed_account::transaction_record::{
        InputDetail, TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};
    let (s, step) = (&f.session, flow::funding_step(draft));
    let spend = f
        .engine
        .block_on(s.stuck_funding_draft(f.wallet, &step))
        .unwrap();
    let seam = Arc::new(SelfSpendSeam::default());
    *spend.broadcaster.lock().unwrap() = Some(seam.clone());
    let grant = (s.vault())
        .authorize(
            GrantPurpose::Spend { max_duffs: 10_000 },
            Some(&f.wallet.0),
            Credential::None,
        )
        .unwrap()
        .id;
    f.engine
        .block_on(s.send_self_spend(&spend, &step, grant))
        .unwrap();
    let tx = seam
        .0
        .lock()
        .unwrap()
        .take()
        .expect("the self-spend was sent");
    let wallet = f.engine.block_on(s.wallet(&f.wallet)).unwrap();
    f.engine.block_on(async {
        let address = (wallet.core())
            .next_receive_address_for_account(0)
            .await
            .unwrap();
        let record = TransactionRecord::new(
            tx,
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            TransactionContext::InChainLockedBlock(BlockInfo::new(
                2,
                dashcore::BlockHash::all_zeros(),
                0,
            )),
            TransactionType::Standard,
            TransactionDirection::Internal,
            vec![InputDetail {
                index: 0,
                value: COIN,
                address,
            }],
            Vec::new(),
            0,
        );
        (wallet.state_mut().await.core_wallet.accounts)
            .standard_bip44_accounts
            .get_mut(&0)
            .expect("BIP44 account 0")
            .transactions_mut()
            .insert(record.txid, record);
    });
    assert!(f.engine.block_on(s.leases.refresh_funding(f.wallet)));
}

/// Gives `draft` (at `KeysPrepared`) the outpoint `lock` at `FundingSent`,
/// as an earlier build or a restored backup left it.
fn give(f: &Fixture, draft: i64, lock: OutPoint) {
    let moved = f
        .appdb()
        .update_registration(
            &f.wallet.to_string(),
            draft,
            f.row(draft).owner,
            Phase::KeysPrepared.as_str(),
            false,
            &RegistrationChange {
                phase: Some(Phase::FundingSent.as_str().into()),
                outpoint: Some(flow::canonical(lock)),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(moved, dw_appdb::RegistrationUpdate::Applied);
}

fn delete(f: &Fixture, draft: i64) {
    let row = f.row(draft);
    assert!(
        f.appdb()
            .delete_registration(
                &f.wallet.to_string(),
                draft,
                row.owner,
                &row.phase,
                row.asset_lock_outpoint.as_deref(),
            )
            .unwrap()
    );
}

/// The view PA-1 lists for `draft`'s step once Repair resolved it (PA-3,
/// OD-02): `NotSent`, its signed `t` a tombstone, the marker flagged
/// (P2b's C4 reading, Q13: claim `None`, so the step is cancelled).
fn resolved(draft: i64, index: u32, t: Txid) -> FundingStepView {
    FundingStepView {
        draft,
        key: FundingKey {
            funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
            identity_index: index,
            account: 0,
        },
        status: Some(DispatchState::NotSent),
        signed: vec![],
        dead: vec![t],
        durable_record: true,
        recording: false,
    }
}

/// Moves the `KeysPrepared` `draft` to `index`, as a restore behind reality
/// left it.
fn move_to(f: &Fixture, draft: i64, index: u32) {
    let moved = f
        .appdb()
        .update_registration(
            &f.wallet.to_string(),
            draft,
            f.row(draft).owner,
            Phase::KeysPrepared.as_str(),
            false,
            &RegistrationChange {
                identity_index: Some(index),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(moved, dw_appdb::RegistrationUpdate::Applied);
}

fn backups(f: &Fixture, draft: i64) -> u32 {
    f.session
        .platform
        .registrations
        .get((f.wallet, draft))
        .backups
}

fn resume_refused(f: &Fixture, lib: &Arc<Fake>, draft: i64) {
    let resumed = f.engine.block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(lib),
        &draft.to_string(),
        None,
    ));
    assert_eq!(resumed.unwrap_err().code(), "invalid_argument");
}

/// Item 4, OD-03 (Sol's r7 fixture): A is open and its step's signed T1
/// came back consumed (another device's IdentityCreate). Until discovery
/// reports the identity, A waits (`Network`, `pending` 1) and nothing is
/// written; B at the identity's index is refused T1. Then "Finish
/// transfers" attaches T1 and the identity at its own index j, and A
/// completes with no funding and no IdentityCreate; a new start is allowed.
#[test]
fn od_03_consumed_signed_value_attaches_once_discovered() {
    let f = fixture(false);
    let lib = f.fake();
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    assert_eq!(lib.counts(), (1, 0));
    lib.w()
        .locks
        .push(registration_lock(t1, 0, LockStatus::Consumed));
    let j = 7;
    let b = f.insert(prepared_row(j));

    // Undiscovered: A waits for the network, holding nothing.
    assert_eq!(f.status(&lib, a).waiting, Some(RegistrationWait::Network));
    let report = finish_once(&f, &lib);
    assert_eq!(report.still_pending, 1, "{report:?}");
    assert!(held_by(&f, t1).is_empty());
    assert_eq!(ledger(&f, &lib).standing(&f.row(a)), Standing::Awaiting);

    // Discovered at j: T1 is A's step's, so B at j is refused it.
    let id = flow::b58(&flow::identity_for(t1));
    lib.w().identities.push((j, id.clone()));
    assert_eq!(
        ledger(&f, &lib).index_owner(&f.row(b), j),
        IndexOwner::Taken
    );
    f.discard(&lib, b).unwrap();
    let report = finish_once(&f, &lib);
    assert_eq!(report.still_pending, 0, "{report:?}");
    let row = f.row(a);
    assert_eq!(
        (
            flow::outpoint_of(&row),
            row.identity.as_deref(),
            row.identity_index,
            phase(&row),
            row.error.as_deref()
        ),
        (Some(t1), Some(id.as_str()), Some(j), Phase::Done, None)
    );
    assert_eq!(lib.counts(), (1, 0), "funded again or created an identity");
    assert!(start(&f, &lib).is_ok());
}

/// Item 5, OD-03 (§5.2c, Q8): a restore brings B′ holding T1 of A's step
/// and A′ open at the step's index (A's own row is gone). The holder keeps
/// T1; A′, the step's heir, closes `Cancelled(funded_elsewhere)`: not
/// committed, never funded, its close backed up, resume refused, and it can
/// be discarded.
#[test]
fn od_03_a_restored_wrong_holder_keeps_the_lock_and_the_signer_closes() {
    let f = fixture(false);
    let lib = f.fake();
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let index = f.row(a).identity_index.unwrap();
    delete(&f, a);
    let b = f.insert(prepared_row(index));
    give(&f, b, t1);
    let a2 = f.insert(prepared_row(index));

    let l = ledger(&f, &lib);
    assert_eq!(
        l.standing(&f.row(a2)),
        Standing::Cancelled(funded_elsewhere())
    );
    assert_eq!(l.standing(&f.row(b)), Standing::Funded(t1));
    assert!(!f.status(&lib, a2).funds_committed);

    // Its run writes the close (§5.2b) and stops.
    f.drive(&lib, a2, Some(f.lease()), false);
    let row = f.row(a2);
    assert_eq!(
        (row.asset_lock_outpoint, row.error.as_deref(), row.retryable),
        (None, Some(funded_elsewhere()), false)
    );
    assert_eq!(lib.counts().0, 1, "A′ funded the closed intent again");
    assert_eq!(
        f.session.platform.registrations.get((f.wallet, a2)).backups,
        1,
        "MN-2: the close is backed up"
    );
    resume_refused(&f, &lib, a2);
    assert_eq!(held_by(&f, t1), [b]);
    f.discard(&lib, a2).unwrap();
}

/// Item 6, OD-04, the part this build runs: §5.2b's write for a row whose
/// outpoint a tombstone voids (the test constructor's view), applied to
/// the real row; after it the code alone keeps the row `Cancelled`, whatever
/// the stub's view says: not committed, `funds_committed` false, resume
/// refused, a new start and the discard allowed.
#[test]
fn od_04_a_voided_outpoint_is_cleared_and_the_close_holds_by_its_code() {
    let f = fixture(false);
    let lib = f.fake();
    let t1 = OutPoint::new(Txid::from_byte_array([0x31; 32]), 0);
    let a = f.insert(prepared_row(3));
    give(&f, a, t1);
    let row = f.row(a);
    let view = FundingStepView {
        draft: a,
        key: FundingKey {
            funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
            identity_index: 3,
            account: 0,
        },
        status: Some(DispatchState::NotSent),
        signed: vec![],
        dead: vec![t1.txid],
        durable_record: true,
        recording: false,
    };
    let l = Ledger::new(vec![], vec![], vec![row.clone()], vec![view], vec![]);
    let reconcile = l.reconcile();
    assert_eq!(
        reconcile,
        [Reconcile::Close {
            row: a,
            clear: true,
            code: Some(funding_cancelled()),
        }]
    );
    assert!(
        flow::apply_reconcile(
            &f.appdb(),
            &f.wallet.to_string(),
            &row,
            row.owner,
            &reconcile[0],
            3,
        )
        .unwrap()
    );
    let row = f.row(a);
    assert_eq!(
        (
            row.asset_lock_outpoint.as_deref(),
            phase(&row),
            row.error.as_deref(),
            row.retryable
        ),
        (None, Phase::KeysPrepared, Some(funding_cancelled()), false)
    );
    // Idempotent.
    let l = Ledger::new(vec![], vec![], vec![row.clone()], vec![], vec![]);
    assert_eq!(l.reconcile(), []);
    assert_eq!(l.standing(&row), Standing::Cancelled(funding_cancelled()));

    let status = f.status(&lib, a);
    assert!(!status.funds_committed);
    resume_refused(&f, &lib, a);
    assert!(start(&f, &lib).is_ok());
    f.discard(&lib, a).unwrap();
}

/// Item 6, OD-04, P2b's real path: a successful tap, tracking lost, and
/// Repair's ChainLocked self-spend of T1's recorded inputs resolves the step
/// `NotSent` with T1 tombstoned. A closes `Cancelled(funding_cancelled)`
/// with its outpoint cleared and its backup re-armed; discard and `start`
/// are allowed; the step is never called again.
#[test]
fn od_04_a_self_spend_closes_the_row_it_voids() {
    let f = fixture(false);
    let lib = f.fake();
    // The call ends without an outcome (`MaybeSent`): a step that read
    // `Sent` is not Repair's.
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().proof_arrives = false;
    lock_on_a_coin(&f, &lib);
    let a = f.insert(core_row(PLAIN));
    f.drive(&lib, a, Some(f.lease()), false);
    let t1 = flow::outpoint_of(&f.row(a)).expect("the tap recorded T1");
    lib.w().locks.clear();
    repair(&f, a);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(steps[0].dead, [t1.txid]);
    let backups = f.session.platform.registrations.get((f.wallet, a)).backups;
    // A had no failure to clear: the close is reconcile's, not resume's.
    finish_once(&f, &lib);
    let row = f.row(a);
    assert_eq!(
        (row.asset_lock_outpoint, row.error.as_deref()),
        (None, Some(funding_cancelled()))
    );
    assert_eq!(
        f.session.platform.registrations.get((f.wallet, a)).backups,
        backups + 1
    );
    assert!(!f.status(&lib, a).funds_committed);
    resume_refused(&f, &lib, a);
    assert_eq!(lib.counts().0, 1);
    assert!(start(&f, &lib).is_ok());
    f.discard(&lib, a).unwrap();
}

/// Item 7, OD-05: between `enter_funding` and P2b's marker, A is calling.
/// A `Free` T2 at A's index goes neither to A through the tap nor through
/// "Finish transfers", which gives it a recovery row; then A's call signs
/// T1, which the tap gives A, calling, and A completes with it.
#[test]
fn od_05_a_calling_row_takes_only_its_own_step_lock() {
    let f = fixture(false);
    let lib = f.fake();
    lib.permit_hold.store(true, Ordering::SeqCst);
    let a = f.insert(core_row(PLAIN));
    f.spawn(&lib, a, Some(f.lease()));
    eventually("A's call waits before its marker", || {
        lib.hung.load(Ordering::SeqCst)
    });
    assert!(f.session.platform.registrations.get((f.wallet, a)).in_call);
    let index = f.row(a).identity_index.unwrap();
    let t2 = OutPoint::new(Txid::from_byte_array([0x42; 32]), 0);
    lib.w()
        .locks
        .push(registration_lock(t2, index, LockStatus::Built));
    assert_eq!(tap(&f, t2, index), None, "the tap gave a calling row T2");
    finish_once(&f, &lib);
    let holders = held_by(&f, t2);
    assert_eq!(holders.len(), 1, "{:?}", f.rows());
    assert_ne!(holders, [a]);
    assert_eq!(flow::outpoint_of(&f.row(a)), None);

    lib.permit_hold.store(false, Ordering::SeqCst);
    f.settle(a);
    let row = f.row(a);
    let t1 = OutPoint::new(Txid::from_byte_array([index as u8 + 1; 32]), 0);
    assert_eq!(
        (flow::outpoint_of(&row), phase(&row), row.error),
        (Some(t1), Phase::Done, None)
    );
    assert_eq!(lib.counts().0, 1);
}

/// Item 7's controls: a marked call with no signed record (`Unknown`)
/// contests T2 at its index, so neither A nor a recovery row takes it; a
/// marked call that signed T1 (`Exact`) gets T1 from "Finish transfers",
/// calling, and never through `Recovery`.
#[test]
fn od_05_controls_marked_unknown_and_exact_calls() {
    let f = fixture(false);
    let lib = f.fake();
    lib.hang_at(Call::Fund, When::Before);
    let a = f.insert(core_row(PLAIN));
    f.spawn(&lib, a, Some(f.lease()));
    eventually("A's call is marked", || lib.hung.load(Ordering::SeqCst));
    let index = f.row(a).identity_index.unwrap();
    let t2 = OutPoint::new(Txid::from_byte_array([0x42; 32]), 0);
    lib.w()
        .locks
        .push(registration_lock(t2, index, LockStatus::Built));
    let report = finish_once(&f, &lib);
    assert!(held_by(&f, t2).is_empty(), "{:?}", f.rows());
    assert_eq!(report.still_pending, 2, "{report:?}");

    let f = fixture(false);
    let lib = f.fake();
    lib.w().skip_tap = true;
    lib.hang_at(Call::Fund, When::Tracked);
    let a = f.insert(core_row(PLAIN));
    f.spawn(&lib, a, Some(f.lease()));
    eventually("A's call signed T1", || lib.hung.load(Ordering::SeqCst));
    let t1 = lib.w().locks[0].outpoint;
    finish_once(&f, &lib);
    assert_eq!(held_by(&f, t1), [a]);
    assert_eq!(f.rows().len(), 1, "a recovery row for the caller's lock");
}

/// Item 8, OD-06: P2b supersedes A's step, the process restarts, and
/// "Finish transfers" runs: the tracked `Built` T1 is a tombstone, so it is
/// `Dead` and no recovery row appears; a call of the step is refused.
#[test]
fn od_06_a_tombstoned_lock_is_dead_after_a_restart() {
    let mut f = fixture(false);
    let lib = f.fake();
    lock_on_a_coin(&f, &lib);
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    repair(&f, a);
    lib.w()
        .locks
        .push(registration_lock(t1, 0, LockStatus::Built));
    reopen(&mut f);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.draft, s.dead.clone()))
            .collect::<Vec<_>>(),
        [(a, vec![t1.txid])]
    );
    finish_once(&f, &lib);
    assert!(held_by(&f, t1).is_empty());
    assert_eq!(f.rows().len(), 1, "a recovery row for a tombstone");
    resume_refused(&f, &lib, a);
    assert_eq!(lib.counts().0, 1, "a cancelled step was called again");
}

/// Item 11, C4: after item 6, an unspent registration lock U with a foreign
/// txid is reconstructed at index 0 (unattributed). P2b reads A's step
/// `MaybeSent`; DP1-02 reads its claim `None` (tombstones, flagged), so U
/// goes to a recovery row and A stays `Cancelled`. Control: unflagged, the
/// step reads `Unknown` and U is `Contested`.
#[test]
fn c4_a_tombstoned_step_does_not_contest_an_unattributed_lock() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().proof_arrives = false;
    lock_on_a_coin(&f, &lib);
    let a = f.insert(core_row(PLAIN));
    f.drive(&lib, a, Some(f.lease()), false);
    lib.w().locks.clear();
    repair(&f, a);
    let u = OutPoint::new(Txid::from_byte_array([0x55; 32]), 0);
    lib.w()
        .locks
        .push(registration_lock(u, 0, LockStatus::RecoveredFromChain));
    finish_once(&f, &lib);
    let holders = held_by(&f, u);
    assert_eq!(holders.len(), 1);
    assert_ne!(holders, [a]);
    assert_eq!(f.row(a).error.as_deref(), Some(funding_cancelled()));
}

/// Item 1's flagged variant, C5: R8-01's lost record under a schema-6
/// flagged marker reads `NotSent` after the reopen; nothing more is funded,
/// and A closes `Cancelled(funding_cancelled)` (Q14), so discard and a new
/// start are allowed.
#[test]
fn r8_01_a_flagged_marker_without_a_record_closes_its_row() {
    let mut f = fixture(false);
    let (draft, _) = unlanded(&mut f, true);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(
        (steps[0].status, steps[0].durable_record),
        (Some(DispatchState::NotSent), true)
    );
    let lib = f.fake();
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(f.row(draft).error.as_deref(), Some(funding_cancelled()));
    assert!(!f.status(&lib, draft).funds_committed);
    assert_eq!(lib.counts().0, 0);
    f.discard(&lib, draft).unwrap();
    assert!(start(&f, &lib).is_ok());
}

/// Item 2, OD-02, the part this build runs: an unflagged marker with no
/// signed record stays `Unknown` for good (§8.1): its row is `Awaiting`
/// (`FundingUnknown`), discard and `start` are refused, nothing is funded,
/// and a lock at its index stays unheld. (The pre-6 dispatch DB opened by
/// the new build is P2b's migration.)
#[test]
fn od_02_an_unflagged_recordless_marker_stays_unknown() {
    let mut f = fixture(false);
    let (draft, _) = unlanded(&mut f, false);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert!(!steps[0].durable_record);
    let lib = f.fake();
    let t2 = OutPoint::new(Txid::from_byte_array([0x52; 32]), 0);
    lib.w()
        .locks
        .push(registration_lock(t2, 0, LockStatus::Built));
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::FundingUnknown)
    );
    assert_eq!(
        f.discard(&lib, draft).unwrap_err().code(),
        "invalid_argument"
    );
    assert!(start(&f, &lib).is_err());
    finish_once(&f, &lib);
    f.drive(&lib, draft, Some(f.lease()), false);
    assert!(held_by(&f, t2).is_empty());
    assert_eq!(lib.counts().0, 0);
}

/// Item 7's control (§5.1 OD-05): a `Free` T2 that the tap or "Finish
/// transfers" gives A after A's funding step read the ledger (`Unfunded`, a
/// free index) and before `enter_funding` (A not calling yet) is A's:
/// `enter_funding` sees the outpoint and steps again without calling the
/// library, and A completes with T2. Its step is never marked.
#[test]
fn od_05_a_free_lock_given_before_enter_funding_is_used_without_a_call() {
    for by_finish in [false, true] {
        let f = fixture(false);
        let lib = f.fake();
        lib.spendable_hold.store(true, Ordering::SeqCst);
        let a = f.insert(core_row(PLAIN));
        f.spawn(&lib, a, Some(f.lease()));
        eventually("A's funding step read the ledger", || {
            lib.hung.load(Ordering::SeqCst)
        });
        assert!(!f.session.platform.registrations.get((f.wallet, a)).in_call);
        let index = f.row(a).identity_index.unwrap();
        let t2 = OutPoint::new(Txid::from_byte_array([0x42; 32]), 0);
        lib.w()
            .locks
            .push(registration_lock(t2, index, LockStatus::Built));
        if by_finish {
            finish_once(&f, &lib);
        } else {
            assert_eq!(tap(&f, t2, index), Some(a), "the tap");
        }
        assert_eq!(held_by(&f, t2), [a], "by Finish: {by_finish}");

        lib.spendable_hold.store(false, Ordering::SeqCst);
        f.settle(a);
        let row = f.row(a);
        assert_eq!(
            (flow::outpoint_of(&row), phase(&row), row.error),
            (Some(t2), Phase::Done, None),
            "by Finish: {by_finish}"
        );
        assert_eq!(
            lib.counts(),
            (0, 1),
            "by Finish: {by_finish}: A funded over the lock it was given"
        );
        assert!(
            f.session
                .leases
                .registration_funding_steps(f.wallet)
                .unwrap()
                .is_empty(),
            "by Finish: {by_finish}: A's step was marked"
        );
    }
}

/// Item 10, I12 (§6 `index_owner`'s step arm): B, restored behind reality,
/// sits at the index of A's step, which Repair resolved (the view PA-1 will
/// list: `NotSent`, T1 tombstoned, flagged). A is `Cancelled` and younger,
/// so no row keeps the index, but the cancelled step does: B's run moves it
/// to a fresh index before it funds, and it completes there. A's step is
/// never called again.
#[test]
fn i12_a_row_at_a_cancelled_steps_index_moves_before_it_funds() {
    let f = fixture(false);
    let lib = f.fake();
    let views = Views::new(&f.session.leases, f.wallet);
    let b = f.insert(prepared_row(9));
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let index = f.row(a).identity_index.unwrap();
    assert_ne!(index, 9);
    views.set(resolved(a, index, t1.txid));
    move_to(&f, b, index);
    let l = ledger(&f, &lib);
    assert_eq!(
        l.standing(&f.row(a)),
        Standing::Cancelled(funding_cancelled())
    );
    assert_eq!(l.standing(&f.row(b)), Standing::Unfunded);
    // Read before the run, checked after it: the run decides first.
    let owner = l.index_owner(&f.row(b), index);

    f.drive(&lib, b, Some(f.lease()), false);
    let row = f.row(b);
    assert_ne!(
        row.identity_index,
        Some(index),
        "B funded on the step's slot"
    );
    assert_eq!(phase(&row), Phase::Done);
    assert_eq!(lib.counts(), (2, 1), "one funding for B, one identity");
    assert_ne!(flow::outpoint_of(&row), Some(t1));
    assert_eq!(f.row(a).identity_index, Some(index));
    assert_eq!(owner, IndexOwner::Taken);
}

/// Item 10's other arm (D14): B, older and restored behind reality, sits at
/// the index of A's outstanding step (A `Awaiting` for its signed T1, away).
/// A's committed row keeps the index, so B moves to a fresh one and funds
/// there, rather than waiting on the step; A keeps its key and still takes
/// T1 when it returns.
#[test]
fn i12_a_row_at_an_outstanding_steps_index_moves_before_it_funds() {
    let f = fixture(false);
    let lib = f.fake();
    let b = f.insert(prepared_row(9));
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let index = f.row(a).identity_index.unwrap();
    move_to(&f, b, index);
    let l = ledger(&f, &lib);
    assert_eq!(l.standing(&f.row(a)), Standing::Awaiting);
    let owner = l.index_owner(&f.row(b), index);

    f.drive(&lib, b, Some(f.lease()), false);
    let row = f.row(b);
    assert_ne!(row.identity_index, Some(index), "B waited on the slot");
    assert_eq!(phase(&row), Phase::Done);
    assert_eq!(lib.counts(), (2, 1));
    assert_eq!(owner, IndexOwner::Taken);

    lib.w()
        .locks
        .push(registration_lock(t1, index, LockStatus::Built));
    finish_once(&f, &lib);
    let row = f.row(a);
    assert_eq!(
        (flow::outpoint_of(&row), row.identity_index, phase(&row)),
        (Some(t1), Some(index), Phase::Done)
    );
    assert_eq!(lib.counts(), (2, 2));
}

/// Item 10 through `next_index` (I12): once A's resolved step closed A and
/// A was discarded, the step's slot stays used. A new start lands past it,
/// funds there and completes; on the slot it would inherit the cancelled
/// step (the heir rule) and close instead.
#[test]
fn i12_a_new_draft_skips_a_cancelled_steps_index() {
    let f = fixture(false);
    let lib = f.fake();
    let views = Views::new(&f.session.leases, f.wallet);
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let index = f.row(a).identity_index.unwrap();
    views.set(resolved(a, index, t1.txid));
    f.discard(&lib, a).unwrap();
    assert!(f.rows().is_empty());
    assert_eq!(ledger(&f, &lib).next_index(), index + 1);

    let c: i64 = start(&f, &lib).unwrap().parse().unwrap();
    f.settle(c);
    let row = f.row(c);
    assert_eq!(
        (row.identity_index, phase(&row), row.error.as_deref()),
        (Some(index + 1), Phase::Done, None)
    );
    assert_eq!(lib.counts(), (2, 1));
}

/// Item 6's control, the ledger's rules (PA-3's retention across
/// `erase_wallet` stands in through [`Views`]): a backup taken while A held
/// its tapped T1, before Repair's self-spend resolved A's step and A closed,
/// is restored after the wallet's rows were removed. The restored A′ records
/// the voided T1; its run clears it and closes A′ the same way
/// (`Cancelled(funding_cancelled)`, backup re-armed); nothing funds again,
/// and discard and a new start are allowed.
#[test]
fn od_04_a_pre_closure_backup_restored_after_a_removal_closes_the_same_way() {
    let f = fixture(false);
    let lib = f.fake();
    let views = Views::new(&f.session.leases, f.wallet);
    // Broadcast, no proof yet: A waits at `ProofWaiting` holding T1.
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().proof_arrives = false;
    let a = f.insert(core_row(PLAIN));
    f.drive(&lib, a, Some(f.lease()), false);
    let t1 = flow::outpoint_of(&f.row(a)).expect("the tap recorded T1");
    assert_eq!(phase(&f.row(a)), Phase::ProofWaiting);
    let index = f.row(a).identity_index.unwrap();
    let w = f.wallet.to_string();
    let mut backup = f.appdb().export_wallet_rows(&w).unwrap();
    backup.retain(|t| t.table == "dp_registration");
    assert_eq!(backup[0].rows.len(), 1);

    // Tracking lost; Repair resolves the step with T1 tombstoned; A closes.
    lib.w().locks.clear();
    views.set(resolved(a, index, t1.txid));
    finish_once(&f, &lib);
    let row = f.row(a);
    assert_eq!(
        (row.asset_lock_outpoint, row.error.as_deref()),
        (None, Some(funding_cancelled()))
    );

    // The wallet's rows go (its steps stay, F6); the backup comes back.
    delete(&f, a);
    assert_eq!(f.appdb().import_wallet_rows(&w, &backup).unwrap(), 1);
    let a2 = f.rows()[0].id;
    assert_ne!(a2, a);
    assert_eq!(flow::outpoint_of(&f.row(a2)), Some(t1));
    let l = ledger(&f, &lib);
    assert_eq!(l.live(&f.row(a2)), None);
    assert_eq!(
        l.standing(&f.row(a2)),
        Standing::Cancelled(funding_cancelled())
    );
    assert!(!l.committed(&f.row(a2)));

    let before = backups(&f, a2);
    f.drive(&lib, a2, Some(f.lease()), false);
    let row = f.row(a2);
    assert_eq!(
        (
            row.asset_lock_outpoint.as_deref(),
            phase(&row),
            row.error.as_deref(),
            row.retryable
        ),
        (None, Phase::KeysPrepared, Some(funding_cancelled()), false)
    );
    assert_eq!(backups(&f, a2), before + 1, "MN-2: the close is backed up");
    assert!(!f.status(&lib, a2).funds_committed);
    assert_eq!(lib.counts().0, 1, "the cancelled intent funded again");
    resume_refused(&f, &lib, a2);
    f.discard(&lib, a2).unwrap();
    assert!(start(&f, &lib).is_ok());
}

/// Item 6's control on P2b's real path: a wallet backup taken while A held
/// its tapped T1, before Repair's self-spend resolved A's step, is restored
/// after a real wallet removal. Backups carry app rows only, and the
/// dispatch journal keeps the resolved step and its tombstone (PA-3): the
/// restored A′ records the voided T1, and its run clears it and closes A′
/// `Cancelled(funding_cancelled)` with its backup re-armed. Nothing funds
/// again, and discard and a new start are allowed.
#[test]
fn od_04_a_pre_closure_backup_restored_after_a_real_removal_closes_the_same_way() {
    let f = fixture(false);
    let lib = f.fake();
    // Broadcast, no proof yet: A waits at `ProofWaiting` holding T1.
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().proof_arrives = false;
    lock_on_a_coin(&f, &lib);
    let a = f.insert(core_row(PLAIN));
    f.drive(&lib, a, Some(f.lease()), false);
    let t1 = flow::outpoint_of(&f.row(a)).expect("the tap recorded T1");
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("pre-closure.dwbackup");
    let pass = || Some(Zeroizing::new(b"bk".to_vec()));
    f.engine
        .block_on(f.session.backup_wallet(f.wallet, path.clone(), pass()))
        .unwrap();

    lib.w().locks.clear();
    repair(&f, a);
    let wipe = (f.session.vault())
        .authorize(GrantPurpose::Wipe, Some(&f.wallet.0), Credential::None)
        .unwrap()
        .id;
    f.engine
        .block_on(f.session.remove_wallet(f.wallet, wipe))
        .unwrap();
    assert!(f.rows().is_empty());
    assert_eq!(
        f.engine
            .block_on(f.session.restore_backup(path, pass()))
            .unwrap(),
        vec![f.wallet]
    );
    assert!(
        f.engine
            .block_on(f.session.leases.refresh_funding(f.wallet))
    );
    let a2 = f.rows()[0].id;
    assert_ne!(a2, a);
    assert_eq!(flow::outpoint_of(&f.row(a2)), Some(t1));
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.draft, s.status, s.dead.clone()))
            .collect::<Vec<_>>(),
        [(a, Some(DispatchState::NotSent), vec![t1.txid])]
    );
    let before = backups(&f, a2);
    f.drive(&lib, a2, Some(f.lease()), false);
    let row = f.row(a2);
    assert_eq!(
        (row.asset_lock_outpoint, row.error.as_deref()),
        (None, Some(funding_cancelled()))
    );
    assert_eq!(backups(&f, a2), before + 1, "MN-2: the close is backed up");
    assert!(!f.status(&lib, a2).funds_committed);
    assert_eq!(lib.counts().0, 1, "the cancelled intent funded again");
    f.discard(&lib, a2).unwrap();
    assert!(start(&f, &lib).is_ok());
}

/// §5.2a with an `Unfunded` B waiting at index j, where discovery reports
/// the identity A's step's consumed T1 created (j ≠ A's index): the attach
/// is A's alone, `{T1, identity, j, IdentityRegistered}`, and B is never
/// given T1. A completes with no funding and no IdentityCreate; afterwards
/// j is taken for B, which moves to a fresh index before it funds.
#[test]
fn od_03_an_attach_beside_an_unfunded_row_at_the_identitys_index() {
    let f = fixture(false);
    let lib = f.fake();
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let a_index = f.row(a).identity_index.unwrap();
    let j = 7;
    assert_ne!(a_index, j);
    lib.w()
        .locks
        .push(registration_lock(t1, a_index, LockStatus::Consumed));
    let b = f.insert(prepared_row(j));
    let id = flow::b58(&flow::identity_for(t1));
    lib.w().identities.push((j, id.clone()));

    let l = ledger(&f, &lib);
    assert_eq!(l.standing(&f.row(a)), Standing::Awaiting);
    assert_eq!(l.standing(&f.row(b)), Standing::Unfunded);
    assert_eq!(
        l.reconcile(),
        [Reconcile::Attach {
            row: a,
            lock: t1,
            identity: id.clone(),
            index: j,
        }]
    );
    // §5.2a's one write, as the run and Finish apply it.
    let row = f.row(a);
    assert!(
        flow::apply_reconcile(
            &f.appdb(),
            &f.wallet.to_string(),
            &row,
            row.owner,
            &l.reconcile()[0],
            3,
        )
        .unwrap()
    );
    let row = f.row(a);
    assert_eq!(
        (
            flow::outpoint_of(&row),
            row.identity.as_deref(),
            row.identity_index,
            phase(&row),
            row.error.as_deref()
        ),
        (
            Some(t1),
            Some(id.as_str()),
            Some(j),
            Phase::IdentityRegistered,
            None
        )
    );
    assert_eq!(held_by(&f, t1), [a]);
    assert_eq!(
        ledger(&f, &lib).index_owner(&f.row(b), j),
        IndexOwner::Taken
    );

    finish_once(&f, &lib);
    assert_eq!(phase(&f.row(a)), Phase::Done);
    assert_eq!(held_by(&f, t1), [a], "B was given T1");
    assert_eq!(lib.counts(), (1, 0), "A funded or created an identity");

    f.drive(&lib, b, Some(f.lease()), false);
    let row = f.row(b);
    assert_ne!(
        row.identity_index,
        Some(j),
        "B funded on the identity's index"
    );
    assert_eq!(phase(&row), Phase::Done);
    assert_ne!(flow::outpoint_of(&row), Some(t1));
    assert_eq!(lib.counts(), (2, 1));
    assert_eq!(f.row(a).identity_index, Some(j));
}

/// Opus review m1 (D4): the tap never hands on a `Consumed` lock. A's
/// step's T1 reported to the tap tracked `Consumed` at A's known index
/// attaches to no row (consumed value goes only through §5.2a); the same
/// lock reported unconsumed is A's.
#[test]
fn the_tap_never_hands_on_a_consumed_lock() {
    let f = fixture(false);
    let lib = f.fake();
    let (a, t1) = signed_then_rejected(&f, &lib, prepared_row(3));
    let consumed = registration_lock(t1, 3, LockStatus::Consumed);
    assert_eq!(consumed.known_index(), Some(3));
    lib.w().locks.push(consumed.clone());
    let tracked = |lock| {
        flow::lock_tracked(
            &f.session.platform.registrations,
            &f.appdb(),
            &f.session.leases,
            f.wallet,
            lock,
        )
        .unwrap()
    };
    assert_eq!(tracked(consumed), None);
    assert!(held_by(&f, t1).is_empty());
    let row = f.row(a);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, Some("platform.broadcast_unknown"))
    );
    assert_eq!(
        tracked(registration_lock(t1, 3, LockStatus::Built)),
        Some(a),
        "the control: unconsumed, it is A's"
    );
}

/// Opus review m2 (I9): a failed `Awaiting` row (its own step outstanding,
/// `broadcast_unknown` set) shows what it waits for rather than no exit:
/// `ChainLock` while its signed T1 is away, `Network` once T1 is consumed
/// and its identity undiscovered. Its exit runs: discovery attaches T1 and
/// the identity, and A completes with no second funding.
#[test]
fn a_failed_awaiting_row_shows_its_wait() {
    let f = fixture(false);
    let lib = f.fake();
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    assert!(!f.running(a));
    let status = f.status(&lib, a);
    assert_eq!(
        (
            status.phase,
            status.failure.as_ref().map(|e| e.code.as_str()),
            status.waiting,
            status.funds_committed
        ),
        (
            RegistrationPhase::Failed,
            Some("platform.broadcast_unknown"),
            Some(RegistrationWait::ChainLock),
            true
        )
    );
    assert_eq!(ledger(&f, &lib).standing(&f.row(a)), Standing::Awaiting);
    resume_refused(&f, &lib, a);

    lib.w()
        .locks
        .push(registration_lock(t1, 0, LockStatus::Consumed));
    let status = f.status(&lib, a);
    assert_eq!(
        (status.phase, status.waiting),
        (RegistrationPhase::Failed, Some(RegistrationWait::Network))
    );

    lib.w()
        .identities
        .push((0, flow::b58(&flow::identity_for(t1))));
    let report = finish_once(&f, &lib);
    assert_eq!(report.still_pending, 0, "{report:?}");
    let status = f.status(&lib, a);
    assert_eq!(
        (status.phase, status.waiting, status.failure),
        (RegistrationPhase::Done, None, None)
    );
    assert_eq!(lib.counts(), (1, 0));
}

/// D15 on the paths (Sol design r2): after A′'s run writes
/// `funded_elsewhere` (item 5's wrong-holder restore), the holder B′
/// completes with T1 and is discarded. A′ is then the step's heir, but a
/// row closed with a cancel code is no step's row: A′ stays
/// `Cancelled(funded_elsewhere)`, and neither "Finish transfers", its run
/// nor a reconcile gives it T1 or T1's identity. The step settles in place,
/// so no orphan blocks a new start (§5.4, I6).
#[test]
fn d15_a_closed_signer_stays_closed_after_its_holder_is_discarded() {
    let f = fixture(false);
    let lib = f.fake();
    let (a, t1) = signed_then_rejected(&f, &lib, core_row(PLAIN));
    let index = f.row(a).identity_index.unwrap();
    delete(&f, a);
    let b = f.insert(prepared_row(index));
    give(&f, b, t1);
    let a2 = f.insert(prepared_row(index));
    f.drive(&lib, a2, Some(f.lease()), false);
    assert_eq!(f.row(a2).error.as_deref(), Some(funded_elsewhere()));

    // B′ completes with T1 (back with its proof), then is discarded.
    lib.w()
        .locks
        .push(registration_lock(t1, index, LockStatus::ChainLocked));
    f.drive(&lib, b, Some(f.lease()), false);
    let id = f.row(b).identity.expect("B′'s identity");
    assert_eq!(phase(&f.row(b)), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
    f.discard(&lib, b).unwrap();

    let l = ledger(&f, &lib);
    assert_eq!(
        l.standing(&f.row(a2)),
        Standing::Cancelled(funded_elsewhere())
    );
    assert_eq!(l.reconcile(), []);
    assert!(l.orphans().is_empty());
    assert!(!l.committed(&f.row(a2)));
    let report = finish_once(&f, &lib);
    assert_eq!(report.still_pending, 0, "{report:?}");
    f.drive(&lib, a2, Some(f.lease()), false);
    let row = f.row(a2);
    assert_eq!(
        (
            row.asset_lock_outpoint.as_deref(),
            row.identity.as_deref(),
            row.identity_index,
            row.error.as_deref(),
            row.retryable
        ),
        (None, None, Some(index), Some(funded_elsewhere()), false)
    );
    assert!(held_by(&f, t1).is_empty());
    assert!(!f.rows().iter().any(|r| r.identity.as_deref() == Some(&id)));
    assert_eq!(lib.counts(), (1, 1), "A′ funded or created an identity");
    let status = f.status(&lib, a2);
    assert_eq!(
        (status.phase, status.waiting, status.funds_committed),
        (RegistrationPhase::Failed, None, false)
    );
    resume_refused(&f, &lib, a2);
    assert!(start(&f, &lib).is_ok());
}
