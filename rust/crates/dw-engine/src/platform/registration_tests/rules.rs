//! Opus-high MN-4: one test per contract rule that a review mutant broke
//! with no test catching it (mutants B, I, N, O, W, G, H, R and F of
//! `/work/scratch/dp1-02-opus/review-probes.patch`).

use super::*;

/// The fixture's [`Fake`] with one twist the library can show but the
/// Fake does not script.
struct Hooked {
    fake: Arc<Fake>,
    /// `fund` reports a failure that tracked its lock as `NotSent`.
    fund_not_sent: bool,
    /// The funded lock gains its InstantSend proof after the call decided
    /// its error and before the flow reads the locks (Sol r6 R6-01).
    proof_before_return: bool,
    /// Another device's registration lock on the seed shows up, rebuilt
    /// from the mempool, while the call runs (Sol r6 R6-03).
    foreign_after_fund: bool,
    /// Platform's profile reads, scripted ahead of the Fake's own.
    profile_reads: Mutex<std::collections::VecDeque<Result<Option<ProfileRecord>, PlatformError>>>,
    /// Runs before a `tracked_locks` read once it returns `true`, then
    /// never again.
    before_locks: Mutex<Option<Box<dyn FnMut() -> bool + Send>>>,
}

impl Hooked {
    fn new(fake: &Arc<Fake>) -> Self {
        Self {
            fake: Arc::clone(fake),
            fund_not_sent: false,
            proof_before_return: false,
            foreign_after_fund: false,
            profile_reads: Mutex::default(),
            before_locks: Mutex::new(None),
        }
    }
}

impl RegistrationLib for Hooked {
    async fn tracked_locks(&self) -> Vec<LockInfo> {
        {
            let mut hook = self.before_locks.lock().unwrap();
            if hook.as_mut().is_some_and(|h| h()) {
                *hook = None;
            }
        }
        self.fake.tracked_locks().await
    }

    async fn identities(&self) -> Vec<(u32, String)> {
        self.fake.identities().await
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        self.fake.spendable().await
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        self.fake.funding_hold()
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        lock_duffs: u64,
    ) -> Result<CallPermit, FundError> {
        self.fake.funding_permit(lease, step, key, lock_duffs).await
    }

    async fn fund(
        &self,
        duffs: u64,
        index: u32,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        let funded = self.fake.fund(duffs, index, permit).await;
        if self.proof_before_return {
            let lock = self.fake.w().locks[0].outpoint;
            self.fake.set_status(lock, LockStatus::InstantSendLocked);
        }
        if self.foreign_after_fund {
            self.fake.w().locks.push(LockInfo {
                outpoint: OutPoint::new(Txid::from_byte_array([99; 32]), 0),
                registration: true,
                identity_index: 0,
                account: 0,
                amount: flow::LOCK_DUFFS,
                status: LockStatus::Broadcast,
            });
        }
        match funded {
            Err(FundError::Failed(e)) if self.fund_not_sent => Err(FundError::NotSent(e)),
            other => other,
        }
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        self.fake.wait_proof(outpoint, timeout).await
    }

    async fn find_identity(&self, outpoint: OutPoint) -> Result<Option<String>, PlatformError> {
        self.fake.find_identity(outpoint).await
    }

    async fn register_identity(
        &self,
        outpoint: OutPoint,
        index: u32,
        lease: &Lease,
    ) -> Result<String, PlatformError> {
        self.fake.register_identity(outpoint, index, lease).await
    }

    async fn register_name(
        &self,
        identity: &str,
        label: &str,
        lease: &Lease,
    ) -> Result<NameOutcome, NameError> {
        self.fake.register_name(identity, label, lease).await
    }

    async fn platform_profile(
        &self,
        identity: &str,
    ) -> Result<Option<ProfileRecord>, PlatformError> {
        let scripted = self.profile_reads.lock().unwrap().pop_front();
        match scripted {
            Some(read) => read,
            None => self.fake.platform_profile(identity).await,
        }
    }

    async fn create_profile(
        &self,
        identity: &str,
        index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        self.fake
            .create_profile(identity, index, profile, lease)
            .await
    }

    async fn update_profile(
        &self,
        identity: &str,
        index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        self.fake
            .update_profile(identity, index, profile, lease)
            .await
    }

    async fn name_credits(&self, label: &str) -> Result<u64, NameError> {
        self.fake.name_credits(label).await
    }

    async fn credit_balance(&self, identity: &str) -> Result<Option<u64>, PlatformError> {
        self.fake.credit_balance(identity).await
    }
}

/// A restored `KeysPrepared` row at `index` with nothing funded.
fn prepared_row(index: u32) -> NewRegistration {
    let mut row = core_row(PLAIN);
    row.identity_index = Some(index);
    row.phase = Phase::KeysPrepared.as_str().into();
    row
}

/// [`prepared_row`] as a newer build wrote it: this build cannot read it.
fn unreadable_row(index: u32) -> NewRegistration {
    let mut row = prepared_row(index);
    row.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":3000000}"#.into();
    row
}

/// The changeset tap's report of the registration lock `outpoint` built
/// for `index`; the draft it moved.
fn tap(f: &Fixture, outpoint: OutPoint, index: u32) -> Option<i64> {
    flow::lock_tracked(
        &f.session.platform.registrations,
        &f.appdb(),
        f.wallet,
        outpoint,
        Some(index),
    )
    .unwrap()
}

fn held_by(f: &Fixture, outpoint: OutPoint) -> Vec<i64> {
    f.rows()
        .into_iter()
        .filter(|r| flow::outpoint_of(r) == Some(outpoint))
        .map(|r| r.id)
        .collect()
}

/// Opus-high MN-4 (W): the tap's lock goes to the row whose funding call
/// runs, not to the first row at its index. A row restored behind reality
/// lands at the index while the newer row's call runs; the lock that call
/// built is the newer row's.
#[test]
fn the_tap_gives_its_lock_to_the_row_whose_funding_call_runs() {
    let f = fixture(false);
    let lib = f.fake();
    lib.hang_at(Call::Fund, When::Before);
    let older = f.insert(prepared_row(9));
    let calling = f.insert(prepared_row(1));
    f.spawn(&lib, calling, Some(f.lease()));
    eventually("the funding call runs", || {
        f.session
            .platform
            .registrations
            .get((f.wallet, calling))
            .in_call
    });
    let moved = f
        .appdb()
        .update_registration(
            &f.wallet.to_string(),
            older,
            f.row(older).owner,
            Phase::KeysPrepared.as_str(),
            false,
            &RegistrationChange {
                identity_index: Some(1),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(moved, dw_appdb::RegistrationUpdate::Applied);
    let lock = lib.lock_at(1, LockStatus::Built);
    assert_eq!(tap(&f, lock, 1), Some(calling));
    assert_eq!(held_by(&f, lock), [calling]);
    let older = f.row(older);
    assert_eq!(
        (phase(&older), older.asset_lock_outpoint),
        (Phase::KeysPrepared, None)
    );
    f.kill(calling);
}

/// Opus-high MN-4 (B), DEC-173 (2): the tap gives a lock to no row while a
/// row this build cannot read, with no outpoint or identity and past
/// `Draft`, sits at its index: that row may have built it. The readable
/// row beside it stays unfunded, and the unreadable row's funds count as
/// committed.
#[test]
fn the_tap_gives_no_lock_while_an_unreadable_row_sits_at_its_index() {
    let f = fixture(false);
    let lib = f.fake();
    let readable = f.insert(prepared_row(1));
    let unreadable = f.insert(unreadable_row(1));
    let lock = lib.lock_at(1, LockStatus::Built);
    assert_eq!(tap(&f, lock, 1), None);
    assert!(held_by(&f, lock).is_empty(), "{:?}", f.rows());
    let row = f.row(readable);
    assert_eq!(
        (phase(&row), row.asset_lock_outpoint),
        (Phase::KeysPrepared, None)
    );
    assert!(f.status(&lib, unreadable).funds_committed);
}

/// Opus-high MN-4 (I), DEC-173 (1): a run whose generation was taken stops
/// at its next write. The generation is taken while the run waits for its
/// proof, between its `ProofWaiting` write and its identity write: the
/// identity write does not apply, the run ends, and the claim that took the
/// row finishes it with the one identity.
#[test]
fn a_run_whose_generation_was_taken_stops_at_its_next_write() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().proof_arrives = false;
    let draft = adopted_lock_row(&f, &lib, 1, LockStatus::Broadcast);
    f.spawn(&lib, draft, Some(f.lease()));
    eventually("the proof wait", || !lib.w().waits.is_empty());
    assert_eq!(phase(&f.row(draft)), Phase::ProofWaiting);
    let raised = f
        .appdb()
        .claim_registration(&f.wallet.to_string(), draft)
        .unwrap()
        .unwrap();
    lib.w().proof_arrives = true;
    eventually("the run whose generation was taken stops", || {
        !f.running(draft)
    });
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.identity.as_deref(), row.owner),
        (Phase::ProofWaiting, None, raised),
        "a write of the lost generation applied"
    );
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (0, 1));
}

/// Opus-high MN-4 (N): "Finish transfers" gives a lock whose index the
/// library lost to a restored row waiting for one, not to one whose
/// funding call runs: that call funds a lock of its own. The lock gets a
/// new row instead.
#[test]
fn finish_gives_no_unindexed_lock_to_a_row_whose_funding_call_runs() {
    let f = fixture(false);
    let lib = f.fake();
    lib.hang_at(Call::Fund, When::Before);
    let calling = f.insert(prepared_row(2));
    f.spawn(&lib, calling, Some(f.lease()));
    eventually("the funding call runs", || {
        f.session
            .platform
            .registrations
            .get((f.wallet, calling))
            .in_call
    });
    let lock = lib.lock_at(0, LockStatus::RecoveredFromChain);
    f.engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    let row = f.row(calling);
    assert_eq!(
        (phase(&row), row.asset_lock_outpoint),
        (Phase::KeysPrepared, None),
        "Finish gave the calling row a lock"
    );
    let holders = held_by(&f, lock);
    assert_eq!(holders.len(), 1, "{:?}", f.rows());
    assert_ne!(holders[0], calling);
    assert_eq!(f.row(holders[0]).label, "");
    f.kill(calling);
}

/// Opus-high MN-4 (O), H12: after a failed funding call a tracked lock
/// means committed, whatever the error's class. A call that reports
/// `NotSent` with its lock still tracked leaves the row funded at that
/// lock, never unfunded, and the flow goes on with it to the end.
#[test]
fn a_tracked_lock_wins_over_a_not_sent_failure() {
    let f = fixture(false);
    let fake = f.fake();
    fake.w().fund_script = Some(FundScript::FailTracked);
    let lib = Hooked {
        fund_not_sent: true,
        ..Hooked::new(&fake)
    };
    let draft = f.insert(core_row(PLAIN));
    let lease = f.lease();
    f.engine
        .block_on(async {
            let claim = f
                .session
                .platform
                .registrations
                .claim(&f.session, (f.wallet, draft))
                .await?
                .expect("the flow is free");
            Driver::new(&f.session, &lib, &claim, Some(lease), true)
                .run()
                .await
        })
        .unwrap();
    let lock = fake.w().locks.first().map(|l| l.outpoint);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::Done, None),
        "the tracked row was unfunded"
    );
    assert_eq!(flow::outpoint_of(&row), lock);
    assert_eq!(fake.counts(), (1, 1));
}

/// Opus-high MN-4 (G): rows restored behind reality may share an index;
/// a flow not yet funded leaves it to an older row there and takes a new
/// one. Both register, each at its own index, the older at the shared one.
#[test]
fn a_shared_index_stays_with_the_older_row() {
    let f = fixture(false);
    let lib = f.fake();
    let older = f.insert(prepared_row(1));
    let newer = f.insert(prepared_row(1));
    f.drive(&lib, newer, Some(f.lease()), false);
    let row = f.row(newer);
    assert_eq!((phase(&row), row.identity_index), (Phase::Done, Some(2)));
    let row = f.row(older);
    assert_eq!(
        (phase(&row), row.identity_index, row.asset_lock_outpoint),
        (Phase::KeysPrepared, Some(1), None)
    );
    f.drive(&lib, older, Some(f.lease()), false);
    let row = f.row(older);
    assert_eq!((phase(&row), row.identity_index), (Phase::Done, Some(1)));
    assert_eq!(lib.counts(), (2, 2));
}

/// Opus-high MN-4 (H): an identity with no lock to prove it is never
/// adopted. One appears at a funded row's index (not the one its lock
/// derives); the row registers the identity its own lock makes.
#[test]
fn a_funded_row_adopts_no_identity_its_lock_does_not_derive() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().proof_arrives = false;
    let draft = adopted_lock_row(&f, &lib, 1, LockStatus::Broadcast);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::ProofWaiting);
    let lock = flow::outpoint_of(&row).unwrap();
    lib.w().identities.push((1, "unproven".into()));
    lib.set_status(lock, LockStatus::ChainLocked);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.identity),
        (Phase::Done, Some(flow::b58(&flow::identity_for(lock))))
    );
    assert_eq!(lib.counts(), (0, 1));
}

/// Opus-high MN-4 (R), DEC-171 (3): a row this build cannot read holds the
/// lock built for its index, which is never given to another row. A
/// readable row at that index does not adopt it as an unheld lock: it
/// takes a new index and funds its own.
#[test]
fn an_unreadable_rows_lock_is_not_unheld_for_a_row_beside_it() {
    let f = fixture(false);
    let lib = f.fake();
    let unreadable = f.insert(unreadable_row(1));
    let lock = lib.lock_at(1, LockStatus::ChainLocked);
    let readable = f.insert(prepared_row(1));
    f.drive(&lib, readable, Some(f.lease()), false);
    let row = f.row(readable);
    assert_eq!((phase(&row), row.identity_index), (Phase::Done, Some(2)));
    assert_ne!(flow::outpoint_of(&row), Some(lock));
    assert!(held_by(&f, lock).is_empty(), "{:?}", f.rows());
    assert!(f.status(&lib, unreadable).funds_committed);
    assert_eq!(lib.counts(), (1, 1));
}

/// Opus-high MN-4 (F), DEC-173: a discard judges again on the rows read in
/// its delete's own job. The tap records a lock for the row after the
/// first look found it unfunded and the discard took its claim: the
/// discard is refused and the row keeps its lock.
#[test]
fn a_discard_refuses_a_row_funded_after_its_first_look() {
    let f = fixture(false);
    let fake = f.fake();
    let draft = f.insert(prepared_row(1));
    let lib = Arc::new(Hooked::new(&fake));
    let lock = Arc::new(Mutex::new(None));
    let hook = {
        let (session, appdb, wallet, fake, lock) = (
            Arc::clone(&f.session),
            f.appdb(),
            f.wallet,
            Arc::clone(&fake),
            Arc::clone(&lock),
        );
        let first = f.row(draft).owner;
        move || {
            // Once the discard holds the claim it raised.
            let row = appdb.registration_row(&wallet.to_string(), draft);
            if row.unwrap().is_none_or(|r| r.owner == first) {
                return false;
            }
            let outpoint = fake.lock_at(1, LockStatus::Built);
            let moved = flow::lock_tracked(
                &session.platform.registrations,
                &appdb,
                wallet,
                outpoint,
                Some(1),
            );
            assert_eq!(moved.unwrap(), Some(draft));
            *lock.lock().unwrap() = Some(outpoint);
            true
        }
    };
    *lib.before_locks.lock().unwrap() = Some(Box::new(hook));
    let discard = f.engine.block_on(registration::discard(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        &draft.to_string(),
    ));
    let lock = lock
        .lock()
        .unwrap()
        .expect("the tap ran inside the discard");
    assert_eq!(
        discard.map_err(|e| e.code()),
        Err("invalid_argument"),
        "the discard deleted a row funded after its first look"
    );
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), flow::outpoint_of(&row)),
        (Phase::FundingSent, Some(lock))
    );
    assert!(f.status(&fake, draft).funds_committed);
}

/// One run of `draft` through `lib`, as "Finish transfers" runs it (no
/// waits).
fn run_once(f: &Fixture, lib: &impl RegistrationLib, draft: i64) -> Result<(), PlatformError> {
    let lease = f.lease();
    f.engine.block_on(async {
        let claim = f
            .session
            .platform
            .registrations
            .claim(&f.session, (f.wallet, draft))
            .await?
            .expect("the flow is free");
        Driver::new(&f.session, lib, &claim, Some(lease), false)
            .run()
            .await
    })
}

/// Sol r6 R6-01: the call's own lock gains its proof between the call's
/// error and the flow's read, at index 0 (where a proved lock's index says
/// nothing), and its outpoint never reached the row. With P2b's signed
/// record the row takes it after the call; without one (the record's
/// write failed, then a restart) the row fails `broadcast_unknown` and
/// "Finish transfers" settles that row with the lock, never a new row
/// beside it. Either way the registration ends, and the wallet is free.
#[test]
fn review_r6_own_lock_proof_race_must_not_wedge() {
    for unsigned in [false, true] {
        let f = fixture(false);
        let fake = f.fake();
        fake.w().fund_script = Some(FundScript::IsTimeout);
        fake.w().skip_tap = true;
        fake.w().skip_signed = unsigned;
        let lib = Hooked {
            proof_before_return: true,
            ..Hooked::new(&fake)
        };
        let draft = f.insert(core_row(PLAIN));
        run_once(&f, &lib, draft).unwrap();
        let lock = fake.w().locks[0].outpoint;
        let after_call = f.row(draft);
        match unsigned {
            false => assert_eq!(flow::outpoint_of(&after_call), Some(lock)),
            true => assert_eq!(
                (flow::outpoint_of(&after_call), after_call.error.as_deref()),
                (None, Some("platform.broadcast_unknown"))
            ),
        }
        let report = f
            .engine
            .block_on(registration::finish(
                &f.session,
                f.wallet,
                &lib,
                f.grant(COIN, 10 * COIN * 1000, Credential::None),
            ))
            .unwrap();
        let original = f.row(draft);
        assert_eq!(f.rows().len(), 1, "Finish made a second row: {report:?}");
        assert_eq!(
            flow::outpoint_of(&original),
            Some(lock),
            "unsigned: {unsigned}"
        );
        assert_eq!(phase(&original), Phase::Done, "unsigned: {unsigned}");
        let start = f.engine.block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&fake),
            request("erin2"),
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ));
        assert!(start.is_ok(), "the wallet stayed blocked: {start:?}");
        assert_eq!(fake.counts(), (1, 1));
    }
}

/// Sol r6 R6-03: another device's registration lock on the seed, rebuilt
/// from the mempool as `Broadcast` at index 0 with this row's amount, shows
/// up while the row's funding call runs and is rejected. No call of the row
/// signed it, so the row does not take it; it waits for "Finish transfers".
/// The rejected lock was signed, so the row's funding may be out (P2b
/// review M-2): it fails `broadcast_unknown`, not retryable, rather than
/// funding again.
#[test]
fn review_r6_foreign_broadcast_during_rejected_call_must_wait_for_finish() {
    let f = fixture(false);
    let fake = f.fake();
    fake.w().fund_script = Some(FundScript::Rejected);
    fake.w().skip_tap = true;
    fake.w().proof_arrives = false;
    let lib = Hooked {
        foreign_after_fund: true,
        ..Hooked::new(&fake)
    };
    let draft = f.insert(core_row(PLAIN));
    run_once(&f, &lib, draft).unwrap();
    let row = f.row(draft);
    assert_eq!(
        row.asset_lock_outpoint, None,
        "another device's lock waits for Finish"
    );
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("platform.broadcast_unknown"), false)
    );
    // A resume adopts nothing and funds nothing.
    let resumed = f.engine.block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(&fake),
        &draft.to_string(),
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    ));
    assert!(resumed.is_err());
    assert_eq!(f.row(draft).asset_lock_outpoint, None);
    assert_eq!(fake.counts(), (1, 0));
}

/// Sol r6 R6-03, the rule: a lock rebuilt from the chain as `Broadcast` at
/// index 0 says nothing of who funded it; without a signed record of the
/// row's funding call it is not the row's.
#[test]
fn review_r6_reconstructed_broadcast_during_failed_call_is_not_own() {
    let f = fixture(false);
    let draft = f.insert(prepared_row(0));
    let row = f.row(draft);
    let lock = LockInfo {
        outpoint: OutPoint::new(Txid::from_byte_array([9; 32]), 0),
        registration: true,
        identity_index: 0,
        account: 0,
        amount: flow::LOCK_DUFFS,
        status: LockStatus::Broadcast,
    };
    let ev = flow::Evidence::new(vec![lock], vec![], vec![row.clone()]);
    assert!(ev.lock_of(&row).is_none());
    assert!(ev.signed_lock(&row).is_none());
}

/// Sol r6 R6-02: the SDK's `ExecutionNotProved` (a proof of the affected
/// state, not of the transition's execution) and `IdentityNonceNotFound`
/// (a lagging node) map to `internal`, which says nothing of a refusal: a
/// profile put that fails so waits, it never settles the row.
#[test]
fn review_r6_real_sdk_proof_and_nonce_errors_must_not_settle() {
    for sdk_error in [
        dash_sdk::Error::ExecutionNotProved("affected state only".into()),
        dash_sdk::Error::IdentityNonceNotFound("node has not indexed this identity yet".into()),
    ] {
        let kind = format!("{sdk_error:?}");
        let mapped = PlatformError::from(sdk_error);
        let f = fixture(false);
        let fake = f.fake();
        fake.w().profile_error = Some(mapped.clone());
        let draft = f.insert(core_row(PLAIN));
        f.drive(&fake, draft, Some(f.lease()), false);
        let row = f.row(draft);
        assert_ne!(phase(&row), Phase::Done, "{kind} ({mapped:?}) settled");
        assert_eq!(row.profile_error, None, "{kind}");
    }
}

/// Sol r6 R6-02: platform-wallet flattens every SDK error of
/// `register_dpns_name` into `InvalidIdentityData` (a timeout after the
/// preorder too), and DP1-03 returns it when its reconciliation lookup is
/// inconclusive: the name step waits, it never settles.
#[test]
fn review_r6_flattened_name_network_error_must_not_settle() {
    let f = fixture(false);
    let fake = f.fake();
    let error = platform_wallet::PlatformWalletError::InvalidIdentityData(
        "Failed to register DPNS name 'alice2': SDK operation timeout 1 secs reached: domain"
            .into(),
    );
    fake.w().name_error = Some(NameError::Platform(PlatformError::from(error)));
    let draft = f.insert(core_row(PLAIN));
    f.drive(&fake, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::NameRequested);
    assert_eq!(row.name_error, None);
}

/// Sol r6 R6-04: after a refused put, a recheck that fails decides nothing:
/// proof, trust and network failures wait, a storage error ends the run, a
/// closed session stops it, and none settles the row.
#[test]
fn review_r6_refused_profile_recheck_must_preserve_errors() {
    for error in [
        PlatformError::ProofInvalid,
        PlatformError::TrustMismatch,
        PlatformError::Unavailable,
        PlatformError::Storage {
            detail: "read store".into(),
        },
        PlatformError::NetworkNotOpen,
    ] {
        let f = fixture(false);
        let fake = f.fake();
        fake.w().profile_error = Some(PlatformError::InvalidArgument {
            detail: "refused put".into(),
        });
        let lib = Hooked::new(&fake);
        lib.profile_reads
            .lock()
            .unwrap()
            .extend([Ok(None), Err(error.clone())]);
        let draft = f.insert(core_row(PLAIN));
        let run = run_once(&f, &lib, draft);
        let row = f.row(draft);
        assert_ne!(phase(&row), Phase::Done, "{error:?}");
        assert_eq!(row.profile_error, None, "{error:?}");
        if matches!(error, PlatformError::Storage { .. }) {
            assert_eq!(run, Err(error));
        }
    }
}

/// Sol r6 R6-04: two reads that say "no profile" (authentic, but older than
/// the put that landed) and a put refused as a duplicate: the step waits
/// for a read that reconciles it, which publishes the profile, unsettled.
#[test]
fn review_r6_stale_absence_must_not_settle_duplicate_profile() {
    let f = fixture(false);
    let fake = f.fake();
    let lock = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let id = flow::b58(&flow::identity_for(lock));
    fake.w().profiles.insert(
        id,
        ProfileRecord {
            display_name: Some("Alice".into()),
            ..Default::default()
        },
    );
    let lib = Hooked::new(&fake);
    lib.profile_reads
        .lock()
        .unwrap()
        .extend([Ok(None), Ok(None)]);
    let draft = f.insert(core_row(PLAIN));
    run_once(&f, &lib, draft).unwrap();
    assert_ne!(phase(&f.row(draft)), Phase::Done);
    run_once(&f, &lib, draft).unwrap();
    let row = f.row(draft);
    assert_eq!((phase(&row), row.profile_error), (Phase::Done, None));
}
