//! Core → Shielded locks as unfinished transfers ([`ShieldLocks`]): listed
//! and finished through the shield flow's own apply, on the same outpoint,
//! with nothing funded.

use std::str::FromStr;

use key_wallet::bip32::DerivationPath;

use super::*;
use crate::platform::lease::FlowOutcome;
use crate::platform::unfinished_conformance::{self as conformance, End, Rig};
use crate::platform::unfinished_locks::{
    self as locks, KindLock, LockFinish, LockKind, LockKindId, LockOrigin, Mark, Marks, Minimum,
    SpentRead, UnfinishedLock, UnfinishedState,
};
use crate::platform::unfinished_shields::ShieldLocks;

/// A lease of `grant` for the shield flow, noting in `begun` that it was
/// asked for.
fn lease_of(
    s: &Arc<NetworkSession>,
    wallet: crate::WalletId,
    grant: String,
    begun: Arc<AtomicBool>,
) -> impl FnOnce() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Lease, PlatformError>> + Send>,
> {
    let s = Arc::clone(s);
    move || {
        Box::pin(async move {
            begun.store(true, Ordering::SeqCst);
            Ok(s.begin_lease(wallet, FlowKind::Shield, &[grant]).await?)
        })
    }
}

/// Finishes the lock at `outpoint` under a lease of a new grant, and
/// whether that lease was asked for.
fn finish(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    outpoint: OutPoint,
) -> (Result<LockFinish, PlatformError>, bool) {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 1));
    let begun = Arc::new(AtomicBool::new(false));
    let asked = Arc::clone(&begun);
    let finished = run(fx, lib, move |s, lib| async move {
        let kind = ShieldLocks::new(&s, wallet, &*lib);
        locks::finish(&kind, outpoint, lease_of(&s, wallet, grant, asked)).await
    });
    (finished, begun.load(Ordering::SeqCst))
}

fn listed(fx: &Fixture, lib: &Arc<FakeShielded>) -> Result<Vec<UnfinishedLock>, PlatformError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        locks::detect(&ShieldLocks::new(&s, wallet, &*lib)).await
    })
}

fn state_of(fx: &Fixture, lib: &Arc<FakeShielded>) -> Vec<(LockOrigin, UnfinishedState)> {
    (listed(fx, lib).unwrap().into_iter())
        .map(|l| (l.origin, l.state))
        .collect()
}

fn answer(lib: &FakeShielded, answer: Result<Applied, PlatformError>) {
    *lib.applies.lock().unwrap() = Some(answer);
}

/// A lock no shield step here signed (a restore's, read back from the
/// chain) is history to 0.1's pending shields, and an unfinished transfer
/// here: finishing it applies the same outpoint, funds nothing, and only
/// the proved apply settles it, once.
#[test]
fn a_restored_shield_lock_is_listed_and_finished_on_its_outpoint() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    assert!(pending_of(&fx, &lib).is_empty(), "0.1 shows it as history");
    assert_eq!(
        listed(&fx, &lib).unwrap(),
        [UnfinishedLock {
            txid: restored.outpoint.txid.to_string(),
            vout: 0,
            kind: LockKindId::Shielded,
            duffs: restored.amount,
            origin: LockOrigin::Restored,
            state: UnfinishedState::Checking,
            remaining_credits: None,
        }]
    );

    answer(&lib, Ok(Applied::Done));
    let before = funding_counts(&lib);
    let (finished, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert!(begun);
    assert_eq!(*lib.applied.lock().unwrap(), [restored.outpoint]);
    assert_eq!(*lib.settles.lock().unwrap(), [restored.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert!(listed(&fx, &lib).unwrap().is_empty());

    let (again, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(again.unwrap(), LockFinish::Applied);
    assert!(!begun, "a finished lock asks for no lease");
    assert_eq!(lib.applied.lock().unwrap().len(), 1, "applied once");
}

/// Tools ▸ Repair ▸ Finish transfers still finishes a stuck move into
/// Shielded while the library cannot spend from the key set: the lock is
/// already committed, so it lists and applies on its outpoint as before,
/// once, funding nothing.
#[test]
fn a_stuck_shield_finishes_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        no_keyset_spends: AtomicBool::new(true),
        ..FakeShielded::default()
    });
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Checking)]
    );

    answer(&lib, Ok(Applied::Done));
    let before = funding_counts(&lib);
    let (finished, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert!(begun);
    assert_eq!(*lib.applied.lock().unwrap(), [restored.outpoint]);
    assert_eq!(*lib.settles.lock().unwrap(), [restored.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert!(listed(&fx, &lib).unwrap().is_empty());
}

/// Platform's unproved "already consumed" and a failure after the lock's
/// key signed both leave the lock unfinished, "Checking", with no settle
/// (SR1).
#[test]
fn an_unproved_answer_leaves_the_lock_checking() {
    let fx = Fixture::new(0);
    let own = lock(1, LockStatus::InstantSendLocked);
    let restored = lock(2, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone(), restored.clone()],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });

    lib.consumed.store(true, Ordering::SeqCst);
    assert_eq!(
        finish(&fx, &lib, own.outpoint).0.unwrap(),
        LockFinish::Checking
    );
    answer(&lib, Ok(Applied::Unknown));
    assert_eq!(
        finish(&fx, &lib, restored.outpoint).0.unwrap(),
        LockFinish::Checking
    );

    assert!(lib.settles.lock().unwrap().is_empty(), "no settle");
    assert_eq!(
        *lib.unknown.lock().unwrap(),
        HashSet::from([own.outpoint.txid, restored.outpoint.txid])
    );
    assert_eq!(
        state_of(&fx, &lib),
        [
            (LockOrigin::ThisDevice, UnfinishedState::Checking),
            (LockOrigin::Restored, UnfinishedState::Checking),
        ]
    );
}

/// A finish cut after its signature resumes on the same outpoint: no new
/// lock, no funding, and the later proved apply settles it.
#[test]
fn an_interrupted_finish_resumes_on_the_same_outpoint() {
    let fx = Fixture::new(0);
    let own = lock(1, LockStatus::ChainLocked);
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone()],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });
    let before = funding_counts(&lib);
    answer(&lib, Ok(Applied::Unknown));
    assert_eq!(
        finish(&fx, &lib, own.outpoint).0.unwrap(),
        LockFinish::Checking
    );
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, own.outpoint).0.unwrap(),
        LockFinish::Applied
    );

    assert_eq!(*lib.applied.lock().unwrap(), [own.outpoint, own.outpoint]);
    assert_eq!(*lib.settles.lock().unwrap(), [own.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert!(listed(&fx, &lib).unwrap().is_empty());
}

/// Below the activation the lock lists "not active", and a finish refuses
/// before asking for a lease: the grant is kept and nothing is applied.
#[test]
fn an_inactive_network_keeps_the_grant() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        version: 11,
        refreshed: 11,
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::NotActive)]
    );
    let (finished, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(
        finished.unwrap(),
        LockFinish::NotApplied {
            error: "shielded.not_active".into()
        }
    );
    assert!(!begun, "no lease, the grant kept");
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// A lock with no proof yet waits: no lease, no apply.
#[test]
fn a_lock_without_a_proof_waits() {
    let fx = Fixture::new(0);
    let unproved = lock(1, LockStatus::Broadcast);
    let lib = Arc::new(FakeShielded {
        locks: vec![unproved.clone()],
        ..FakeShielded::default()
    });
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Confirming)]
    );
    let (finished, begun) = finish(&fx, &lib, unproved.outpoint);
    assert_eq!(finished.unwrap(), LockFinish::ProofWaiting);
    assert!(!begun);
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// A refusal before the lock's key signed reached nothing: not applied,
/// not marked, still ready.
#[test]
fn a_finish_refused_before_its_signature_is_not_applied() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::ChainLocked);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    answer(&lib, Err(PlatformError::Unavailable));
    assert_eq!(
        finish(&fx, &lib, restored.outpoint).0.unwrap(),
        LockFinish::NotApplied {
            error: "platform.unavailable".into()
        }
    );
    assert!(lib.unknown.lock().unwrap().is_empty());
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Ready)]
    );
}

/// Two finishes of one lock at once apply it once: the second reads it
/// settled under the wallet's shield lock.
#[test]
fn two_finishes_of_one_lock_apply_once() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    answer(&lib, Ok(Applied::Done));
    let wallet = fx.wallet;
    let (g1, g2) = (shield_grant(&fx, 1), shield_grant(&fx, 1));
    let begun = Arc::new(AtomicBool::new(false));
    let outpoint = restored.outpoint;
    let (a, b) = run(&fx, &lib, move |s, lib| async move {
        let kind = ShieldLocks::new(&s, wallet, &*lib);
        tokio::join!(
            locks::finish(
                &kind,
                outpoint,
                lease_of(&s, wallet, g1, Arc::clone(&begun))
            ),
            locks::finish(&kind, outpoint, lease_of(&s, wallet, g2, begun)),
        )
    });
    assert_eq!(
        (a.unwrap(), b.unwrap()),
        (LockFinish::Applied, LockFinish::Applied)
    );
    assert_eq!(lib.applied.lock().unwrap().len(), 1);
    assert_eq!(lib.settles.lock().unwrap().len(), 1);
}

/// Nothing is told apart before the dispatch journal is read, and a lock
/// the wallet does not track is no transfer of it.
#[test]
fn an_unread_journal_or_an_unknown_lock_is_refused() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let unread = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        own: None,
        ..FakeShielded::default()
    });
    assert_eq!(
        listed(&fx, &unread).unwrap_err(),
        PlatformError::Unavailable
    );
    let (finished, begun) = finish(&fx, &unread, restored.outpoint);
    assert_eq!(finished.unwrap_err(), PlatformError::Unavailable);
    assert!(!begun);

    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    let other = OutPoint::new(restored.outpoint.txid, 1);
    let (finished, begun) = finish(&fx, &lib, other);
    assert!(
        matches!(finished, Err(PlatformError::InvalidArgument { .. })),
        "{finished:?}"
    );
    assert!(!begun);
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// The library's own rows: a restored shield lock the restore scan
/// rebuilt (`RecoveredFromChain`, ChainLock proof) lists as a restored
/// transfer, and its credit script, which the finish's signer is narrowed
/// to, is the lock payload's credit output of that outpoint and no other.
#[test]
fn a_restored_library_row_lists_with_its_credit_script() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let transaction = lock_transaction(5);
    let outpoint = OutPoint::new(transaction.txid(), 0);
    let row = TrackedAssetLock {
        operation_id: None,
        shielded_recipient: None,
        out_point: outpoint,
        proof: Some(AssetLockProof::Chain(ChainAssetLockProof::new(
            100,
            outpoint.into(),
        ))),
        transaction,
        account_index: 0,
        funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
        identity_index: 0,
        amount: 1_000_000,
        status: AssetLockStatus::RecoveredFromChain,
    };
    fx.engine.block_on(lib.track(row, true));

    assert_eq!(
        listed(&fx, &lib).unwrap(),
        [UnfinishedLock {
            txid: outpoint.txid.to_string(),
            vout: 0,
            kind: LockKindId::Shielded,
            duffs: 1_000_000,
            origin: LockOrigin::Restored,
            state: UnfinishedState::Checking,
            remaining_credits: None,
        }]
    );
    let scripts = fx.engine.block_on(async {
        let real = lib.real().await;
        (
            real.credit_script(outpoint).await,
            real.credit_script(OutPoint::new(outpoint.txid, 1)).await,
            real.credit_script(lock(5, LockStatus::Broadcast).outpoint)
                .await,
        )
    });
    assert_eq!(scripts, (Some(core_address(5).script_pubkey()), None, None));
}

/// Two credit outputs of one lock transaction are two locks: a proved
/// apply of the first records its outpoint, nothing by txid (0.1's records
/// would speak for the second), and the second stays listed, as an output
/// this version can't finish.
#[test]
fn finishing_one_output_of_a_lock_leaves_its_other_output_listed() {
    let fx = Fixture::new(0);
    let first = lock(1, LockStatus::RecoveredFromChain);
    let second = LockInfo {
        outpoint: OutPoint::new(first.outpoint.txid, 1),
        ..first.clone()
    };
    let lib = Arc::new(FakeShielded {
        locks: vec![first.clone(), second.clone()],
        ..FakeShielded::default()
    });
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, first.outpoint).0.unwrap(),
        LockFinish::Applied
    );
    assert!(lib.settles.lock().unwrap().is_empty(), "no settle by txid");
    let (settled, _) = marks(&fx, &lib);
    assert_eq!(settled.outpoints, HashSet::from([first.outpoint]));
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Unsupported)]
    );
    assert_eq!(listed(&fx, &lib).unwrap()[0].vout, 1);
}

/// The kind's records, as a finish reads them.
fn marks(fx: &Fixture, lib: &Arc<FakeShielded>) -> (Marks, Marks) {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        let kind = ShieldLocks::new(&s, wallet, &*lib);
        (kind.settled_locks().await, kind.unknown_locks().await)
    })
}

/// The credit key the rigs' locks pay, and its P2PKH script as the vault's
/// `Shield` lease derives it.
const LOCK_KEY: &str = "m/9'/1'/5'/5'/0";

fn lock_script(fx: &Fixture) -> dashcore::ScriptBuf {
    let (s, wallet, grant) = (Arc::clone(&fx.s), fx.wallet, shield_grant(fx, 1));
    fx.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant]).await).unwrap();
        let funding = lease.uncounted_funding_signer().unwrap();
        let path = DerivationPath::from_str(LOCK_KEY).unwrap();
        let key = key_wallet::Signer::public_key(&funding, &path)
            .await
            .unwrap();
        dashcore::ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash())
    })
}

/// `ShieldLocks` over the library's own tracked rows (`real_rows`: the
/// funder's wallet, read through the real `PwShielded`, which builds the
/// kind's rows), its apply scripted, its records the session's own
/// `app.sqlite` and journal.
struct ShieldRig {
    fx: Fixture,
    lib: Arc<FakeShielded>,
    script: dashcore::ScriptBuf,
    outcomes: Mutex<Vec<FlowOutcome>>,
}

impl ShieldRig {
    fn new() -> Self {
        let fx = Fixture::new(0);
        let lib = real_rows(&fx);
        let script = lock_script(&fx);
        Self {
            fx,
            lib,
            script,
            outcomes: Mutex::default(),
        }
    }
}

impl Rig for ShieldRig {
    fn funding_type(&self) -> AssetLockFundingType {
        AssetLockFundingType::AssetLockShieldedAddressTopUp
    }

    fn credit_script(&self) -> dashcore::ScriptBuf {
        self.script.clone()
    }

    fn track(&self, rows: Vec<TrackedAssetLock>) {
        for row in rows {
            self.fx.engine.block_on(self.lib.track(row, true));
        }
    }

    fn record_txid(&self, mark: Mark, txid: Txid) {
        run(&self.fx, &self.lib, move |_, lib| async move {
            match mark {
                Mark::Settled => lib.settle(txid).await.unwrap(),
                Mark::Unknown => lib.mark_unknown(txid).await,
                Mark::Used | Mark::Remaining => {
                    unreachable!("no record by txid of a proved read")
                }
            }
        });
    }

    fn next(&self, end: End) {
        let key = DerivationPath::from_str(LOCK_KEY).unwrap();
        if end != End::RefusedUnsigned {
            *self.lib.sign_next.lock().unwrap() = vec![key];
        }
        match end {
            End::Done => answer(&self.lib, Ok(Applied::Done)),
            End::AlreadyConsumed => self.lib.consumed.store(true, Ordering::SeqCst),
            End::Unknown => answer(&self.lib, Ok(Applied::Unknown)),
            End::RefusedUnsigned => answer(&self.lib, Err(PlatformError::Unavailable)),
            End::SignedThenFailed => answer(
                &self.lib,
                Err(PlatformError::Internal {
                    detail: "broadcast failed".into(),
                }),
            ),
        }
    }

    fn applies(&self) -> usize {
        self.lib.applied.lock().unwrap().len()
    }

    fn list(&self) -> Result<Vec<UnfinishedLock>, PlatformError> {
        listed(&self.fx, &self.lib)
    }

    fn finish(&self, outpoint: OutPoint) -> (Result<LockFinish, PlatformError>, bool) {
        let (wallet, grant) = (self.fx.wallet, shield_grant(&self.fx, 1));
        let (kept, begun) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
        let (keep, asked) = (Arc::clone(&kept), Arc::clone(&begun));
        let finished = run(&self.fx, &self.lib, move |s, lib| async move {
            let kind = ShieldLocks::new(&s, wallet, &*lib);
            let leases = Arc::clone(&s);
            let lease_of = || async move {
                asked.store(true, Ordering::SeqCst);
                let lease = leases
                    .begin_lease(wallet, FlowKind::Shield, &[grant])
                    .await?;
                *keep.lock().unwrap() = Some(lease.clone());
                Ok::<_, PlatformError>(lease)
            };
            locks::finish(&kind, outpoint, lease_of).await
        });
        let kept: Option<Lease> = kept.lock().unwrap().take();
        *self.outcomes.lock().unwrap() =
            (kept.as_ref()).map_or_else(Vec::new, conformance::call_outcomes);
        (finished, begun.load(Ordering::SeqCst))
    }

    fn outcomes(&self) -> Vec<FlowOutcome> {
        self.outcomes.lock().unwrap().clone()
    }

    fn marks(&self) -> (Marks, Marks) {
        marks(&self.fx, &self.lib)
    }

    fn minimum(&self) -> Option<Minimum> {
        let wallet = self.fx.wallet;
        run(&self.fx, &self.lib, move |s, lib| async move {
            ShieldLocks::new(&s, wallet, &*lib).minimum()
        })
    }

    /// The conformance run's locks are restored ones.
    fn reports_used(&self) -> bool {
        true
    }

    fn recovers_remaining(&self) -> bool {
        true
    }

    fn prove_spent(&self, outpoint: OutPoint, read: SpentRead) {
        self.lib.spent_reads.lock().unwrap().insert(outpoint, read);
    }
}

/// #402: a shield refused after a penalty, its apply failed after the
/// lock key signed, is read again from Platform's proved state. Partly used
/// with at least Platform's fee for moving the rest, it lists Recoverable
/// with what is left; below that fee, too small; never Checking, and never
/// applied as a shield again. A failure Platform does not prove partly used
/// stays Checking. A partly used lock is no longer a pending shield (whose
/// Check again would shield it again).
#[test]
fn a_shield_refused_after_a_penalty_lists_what_is_left() {
    let rig = ShieldRig::new();
    let floor = {
        let rules = dpp::version::PlatformVersion::latest();
        crate::platform::platform_fund::fee_credits(rules)
    };
    let tx = |n| conformance::transaction(n, 1, 1_000_000, &rig.script);
    let row = |n| {
        let row = conformance::tracked(
            &tx(n),
            0,
            AssetLockFundingType::AssetLockShieldedAddressTopUp,
            AssetLockStatus::ChainLocked,
            true,
        );
        rig.track(vec![row.clone()]);
        row.out_point
    };
    let (left, small, plain) = (row(0xC1), row(0xC2), row(0xC3));
    rig.prove_spent(
        left,
        SpentRead::Remaining {
            credits: 600_000_000,
        },
    );
    rig.prove_spent(small, SpentRead::Remaining { credits: floor - 1 });
    for at in [left, small, plain] {
        rig.next(End::SignedThenFailed);
        assert_eq!(rig.finish(at).0.unwrap(), LockFinish::Checking);
    }
    let listed = rig.list().unwrap();
    let of = |at: OutPoint| {
        let l = (listed.iter())
            .find(|l| l.txid == at.txid.to_string())
            .unwrap();
        (l.state, l.remaining_credits)
    };
    assert_eq!(of(left), (UnfinishedState::Recoverable, Some(600_000_000)));
    assert_eq!(of(small), (UnfinishedState::TooSmall, Some(floor - 1)));
    assert_eq!(of(plain), (UnfinishedState::Checking, None));
    let open = run(&rig.fx, &rig.lib, |_, lib| async move {
        crate::platform::shielded_pool::open_shields(&*lib).await
    })
    .unwrap();
    let open: Vec<_> = open.iter().map(|o| o.lock.outpoint).collect();
    assert_eq!(open, [plain], "a partly used lock is no pending shield");

    let applies = rig.applies();
    for at in [left, small] {
        let (again, asked) = rig.finish(at);
        let refused = LockFinish::NotApplied {
            error: locks::PARTLY_USED.into(),
        };
        assert_eq!(again.unwrap(), refused);
        assert!(!asked, "refused before the lease");
    }
    assert_eq!(rig.applies(), applies, "never applied as a shield again");
}

/// Type 5 holds the contract every kind does (`unfinished_conformance`).
#[test]
fn shield_locks_conform() {
    let rig = ShieldRig::new();
    conformance::conformance(&rig);
    assert!(
        rig.lib.signed.lock().unwrap().iter().all(|s| *s),
        "every signature the rig asked for was the lock key's"
    );
}

/// A lock transaction of two credit outputs paying `n` and `n + 1`, for
/// the real library's rows.
fn two_output_transaction(n: u8) -> dashcore::Transaction {
    use dashcore::transaction::special_transaction::TransactionPayload;
    let mut transaction = lock_transaction(n);
    if let Some(TransactionPayload::AssetLockPayloadType(payload)) =
        &mut transaction.special_transaction_payload
    {
        payload.credit_outputs.push(dashcore::TxOut {
            value: 1_000_000,
            script_pubkey: core_address(n + 1).script_pubkey(),
        });
    }
    transaction
}

/// The real library's restored row of `transaction`'s output `vout`.
fn restored_row(transaction: &dashcore::Transaction, vout: u32) -> TrackedAssetLock {
    let outpoint = OutPoint::new(transaction.txid(), vout);
    TrackedAssetLock {
        operation_id: None,
        shielded_recipient: None,
        out_point: outpoint,
        proof: Some(AssetLockProof::Chain(ChainAssetLockProof::new(
            100,
            outpoint.into(),
        ))),
        transaction: transaction.clone(),
        account_index: 0,
        funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
        identity_index: 0,
        amount: 1_000_000,
        status: AssetLockStatus::RecoveredFromChain,
    }
}

/// Closes the fixture's session and opens it again over the same
/// `app.sqlite`, `wallet.sqlite` and journal (a restart).
fn reopen(fx: &mut Fixture, lib: &Arc<FakeShielded>) {
    (fx.engine)
        .block_on(fx.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let options = crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    };
    fx.s = (fx.engine)
        .block_on(fx.engine.open_network(DashNetwork::Regtest, options))
        .unwrap();
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
    assert!(fx.engine.block_on(fx.s.leases.refresh_funding(fx.wallet)));
}

/// Design test 12: Checking and Done of an output of a shared transaction
/// survive a restart (a new session over the same `app.sqlite` and
/// `wallet.sqlite`), the library's row unchanged as it is when SPV sees
/// the lock transaction again; the untouched sibling keeps its own state.
#[test]
fn a_shared_transactions_outputs_keep_their_state_across_a_restart() {
    let mut fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let (checking, done) = (two_output_transaction(5), two_output_transaction(7));
    for transaction in [&checking, &done] {
        for vout in [0, 1] {
            fx.engine
                .block_on(lib.track(restored_row(transaction, vout), true));
        }
    }
    let at = |t: &dashcore::Transaction, vout| OutPoint::new(t.txid(), vout);
    answer(&lib, Ok(Applied::Unknown));
    assert_eq!(
        finish(&fx, &lib, at(&checking, 0)).0.unwrap(),
        LockFinish::Checking
    );
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, at(&done, 0)).0.unwrap(),
        LockFinish::Applied
    );
    let states = |fx: &Fixture| {
        let mut got: Vec<_> = (listed(fx, &lib).unwrap().into_iter())
            .map(|l| (l.txid, l.vout, l.state))
            .collect();
        got.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        got
    };
    let mut expected = vec![
        (checking.txid().to_string(), 0, UnfinishedState::Checking),
        (checking.txid().to_string(), 1, UnfinishedState::Unsupported),
        (done.txid().to_string(), 1, UnfinishedState::Unsupported),
    ];
    expected.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    assert_eq!(states(&fx), expected, "before the restart");
    reopen(&mut fx, &lib);
    assert_eq!(states(&fx), expected, "after the restart");
}

/// Q2 end to end on type 5: a restored library row whose credit output
/// pays the vault's lock key, finished through `ShieldLocks`, the
/// library's own credit script (`PwShielded::credit_script`) and the
/// shield's `Witnessed` signer: only the lock key signs; a Core address
/// key and another lock's credit key are refused by the narrowing, an
/// identity authentication key and an operator key by the `Shield` lease's
/// vault scope. (The library's apply itself needs Platform;
/// the fake signs where it would.)
#[test]
fn a_type_5_finish_signs_with_the_lock_key_only() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let mut transaction = lock_transaction(9);
    if let Some(
        dashcore::transaction::special_transaction::TransactionPayload::AssetLockPayloadType(
            payload,
        ),
    ) = &mut transaction.special_transaction_payload
    {
        payload.credit_outputs[0].script_pubkey = lock_script(&fx);
    }
    let row = restored_row(&transaction, 0);
    let outpoint = row.out_point;
    fx.engine.block_on(lib.track(row, true));
    let path = |p: &str| DerivationPath::from_str(p).unwrap();
    *lib.sign_next.lock().unwrap() = vec![
        path("m/44'/1'/0'/0/0"),
        path("m/9'/1'/5'/5'/1"),
        path("m/9'/1'/5'/0'/0'/0'/0'"),
        path("m/9'/1'/3'/3'/0'"),
        path(LOCK_KEY),
    ];
    answer(&lib, Ok(Applied::Done));
    assert_eq!(finish(&fx, &lib, outpoint).0.unwrap(), LockFinish::Applied);
    assert_eq!(
        *lib.signed.lock().unwrap(),
        [false, false, false, false, true]
    );
}

/// MIX-SHIELD's drain lock: funded under the shield's own step
/// (`shield/<k>/funding`, the funding permit and `record_signed_locks`
/// path) from up to 50 CoinJoin-account outpoints, its credit output paying
/// the 5'/5' lock key. Its journal row counts it as this device's, it is
/// listed once, and a finish signs with its own credit key only.
#[test]
fn a_drain_lock_lists_once_as_this_devices_and_finishes_with_its_own_key() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let coinjoin: Vec<OutPoint> = (0..50)
        .map(|i| OutPoint::new(Txid::from_byte_array([0xC7; 32]), i))
        .collect();
    *lib.funds_next.lock().unwrap() = Some((coinjoin, lock_script(&fx)));
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    let (outpoint, state) = shield_on(&fx, &lib);
    assert_eq!(state, ShieldState::ConsumptionUnknown);

    let own = fx.s.leases.shield_lock_txids(fx.wallet).unwrap();
    assert!(
        own.contains(&outpoint.txid),
        "the shield step's signed lock"
    );
    let rows = listed(&fx, &lib).unwrap();
    assert_eq!(rows.len(), 1, "listed once: {rows:?}");
    assert_eq!(
        (rows[0].txid.clone(), rows[0].vout, rows[0].origin),
        (outpoint.txid.to_string(), 0, LockOrigin::ThisDevice)
    );
    assert_eq!(rows[0].state, UnfinishedState::Checking);

    let path = |p: &str| DerivationPath::from_str(p).unwrap();
    *lib.sign_next.lock().unwrap() = vec![
        path("m/44'/1'/0'/0/0"),
        path("m/9'/1'/5'/5'/1"),
        path(LOCK_KEY),
    ];
    answer(&lib, Ok(Applied::Done));
    assert_eq!(finish(&fx, &lib, outpoint).0.unwrap(), LockFinish::Applied);
    assert_eq!(*lib.signed.lock().unwrap(), [false, false, true]);
    assert!(listed(&fx, &lib).unwrap().is_empty());
}

/// A restored library row of another device's lock still in flight
/// (`InstantSendLocked`, no proof) lists Confirming and offers no finish.
#[test]
fn a_restored_instant_send_row_without_a_proof_lists_confirming() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let mut row = restored_row(&lock_transaction(6), 0);
    row.status = AssetLockStatus::InstantSendLocked;
    row.proof = None;
    let outpoint = row.out_point;
    fx.engine.block_on(lib.track(row, true));
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Confirming)]
    );
    let (finished, begun) = finish(&fx, &lib, outpoint);
    assert_eq!(finished.unwrap(), LockFinish::ProofWaiting);
    assert!(!begun);
}

/// A lock below the shield's pool fee lists apart, too small to finish,
/// and a finish of it is refused before the lease, so the same grant then
/// finishes another lock.
#[test]
fn a_lock_below_the_fee_is_too_small_and_keeps_the_grant() {
    let fx = Fixture::new(0);
    let dust = LockInfo {
        amount: 1,
        ..lock(1, LockStatus::RecoveredFromChain)
    };
    let normal = lock(2, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![dust.clone(), normal.clone()],
        ..FakeShielded::default()
    });
    assert_eq!(
        state_of(&fx, &lib),
        [
            (LockOrigin::Restored, UnfinishedState::TooSmall),
            (LockOrigin::Restored, UnfinishedState::Checking),
        ]
    );
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let begun = Arc::new(AtomicBool::new(false));
    let finish_with = |outpoint, grant: String| {
        let begun = Arc::clone(&begun);
        run(&fx, &lib, move |s, lib| async move {
            let kind = ShieldLocks::new(&s, wallet, &*lib);
            locks::finish(&kind, outpoint, lease_of(&s, wallet, grant, begun)).await
        })
    };
    let refused = finish_with(dust.outpoint, grant.clone()).unwrap();
    let min = pool::shield_pool_fee(version(LATEST)).unwrap() / 1_000 + 1;
    assert_eq!(
        refused,
        LockFinish::NotApplied {
            error: ShieldedError::BelowMinimum { min, max: None }.code().into()
        }
    );
    assert!(!begun.load(Ordering::SeqCst), "the grant is not redeemed");
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish_with(normal.outpoint, grant).unwrap(),
        LockFinish::Applied,
        "the same grant still works"
    );
    assert_eq!(*lib.applied.lock().unwrap(), [normal.outpoint]);
}

/// A grant with no funding budget is refused before it is redeemed (the
/// finish's funding signer needs one): it is still unredeemed after, and
/// still begins a lease.
#[test]
fn a_grant_without_a_funding_budget_is_kept() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    let purpose = dw_vault::GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 1,
    };
    let granted = (fx.s.vault).authorize(purpose, Some(&fx.wallet.0), dw_vault::Credential::None);
    let (wallet, grant) = (fx.wallet, granted.unwrap().id);
    let outpoint = restored.outpoint;
    let kept = grant.clone();
    let finished = run(&fx, &lib, move |s, lib| async move {
        let kind = ShieldLocks::new(&s, wallet, &*lib);
        let lease = || crate::platform::unfinished::authority(&s, wallet, &grant, FlowKind::Shield);
        locks::finish(&kind, outpoint, lease).await
    });
    assert_eq!(
        finished.unwrap_err(),
        PlatformError::NeedsGrant {
            purpose: crate::platform::flows::BudgetPurpose::Funding
        }
    );
    assert!(lib.applied.lock().unwrap().is_empty());
    let s = Arc::clone(&fx.s);
    let again = fx
        .engine
        .block_on(async move { s.begin_lease(wallet, FlowKind::Shield, &[kept]).await });
    assert!(again.is_ok(), "the grant is still unredeemed: {again:?}");
}

/// Finish transfers' counts over the outpoint evidence: a lock already
/// settled counts in no field; one this version can't finish, one too
/// small and one with no proof are pending, not tried; the finishable
/// output of a shared transaction is tried and completed.
#[test]
fn finish_transfers_counts_by_outpoint() {
    let fx = Fixture::new(0);
    let first = lock(1, LockStatus::RecoveredFromChain);
    let second = LockInfo {
        outpoint: OutPoint::new(first.outpoint.txid, 1),
        ..first.clone()
    };
    let settled = lock(2, LockStatus::RecoveredFromChain);
    let unproved = KindLock {
        info: lock(3, LockStatus::InstantSendLocked),
        proof: false,
        credit_outputs: 1,
    };
    let shared = |info: &LockInfo| KindLock {
        info: info.clone(),
        proof: true,
        credit_outputs: 2,
    };
    let lib = Arc::new(FakeShielded::default());
    *lib.lock_rows.lock().unwrap() = Some(vec![
        shared(&first),
        shared(&second),
        KindLock {
            info: settled.clone(),
            proof: true,
            credit_outputs: 1,
        },
        unproved,
        KindLock {
            info: LockInfo {
                amount: 1,
                ..lock(4, LockStatus::RecoveredFromChain)
            },
            proof: true,
            credit_outputs: 1,
        },
    ]);
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, settled.outpoint).0.unwrap(),
        LockFinish::Applied
    );
    answer(&lib, Ok(Applied::Done));
    let report = finish_transfers(&fx, &lib);
    assert_eq!(
        report.by_kind,
        [locks::TransfersReport {
            kind: LockKindId::Shielded,
            resumed: 1,
            completed: 1,
            still_pending: 3,
        }]
    );
    assert_eq!(totals(&report), (1, 1, 3));
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [settled.outpoint, first.outpoint]
    );
}

/// Repair's "Finish transfers" under its one lease: each unfinished lock
/// once, in order, a restored lock Platform reports used counted as such
/// while the next runs; one with no proof is pending, not tried.
#[test]
fn finish_transfers_runs_each_lock_once_and_reports_the_kind() {
    let fx = Fixture::new(0);
    let (used, applied, unproved) = (
        lock(1, LockStatus::RecoveredFromChain),
        lock(2, LockStatus::RecoveredFromChain),
        lock(3, LockStatus::Broadcast),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![unproved.clone(), applied.clone(), used.clone()],
        ..FakeShielded::default()
    });
    // In txid order: the first apply hears "already consumed", which
    // Platform's proof confirms, the second is proved.
    lib.consumed.store(true, Ordering::SeqCst);
    lib.spent_reads
        .lock()
        .unwrap()
        .insert(used.outpoint, SpentRead::Consumed);
    answer(&lib, Ok(Applied::Done));
    let before = funding_counts(&lib);
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let report = run(&fx, &lib, move |s, lib| async move {
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[grant])
            .await)
            .unwrap();
        let mut report = super::super::FinishReport::default();
        locks::finish_kind(&ShieldLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    });
    assert_eq!(
        report,
        super::super::FinishReport {
            resumed: 2,
            completed: 1,
            still_pending: 1,
            already_used: 1,
            by_kind: vec![locks::TransfersReport {
                kind: LockKindId::Shielded,
                resumed: 2,
                completed: 1,
                still_pending: 1,
            }],
            unread: Vec::new(),
        }
    );
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [used.outpoint, applied.outpoint]
    );
    assert_eq!(*lib.settles.lock().unwrap(), [applied.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Confirming)]
    );
}

/// Finish all tries a Ready lock and a restore's lock read back from the
/// chain (Checking: Platform may already have taken it); this device's
/// lock already Checking before the bulk (an earlier apply may have
/// executed) is pending, not tried; its own "Check again"
/// (`finish_transfer`) still runs it.
#[test]
fn finish_transfers_leaves_a_checking_lock_to_check_again() {
    let fx = Fixture::new(0);
    let (ready, restored, checking) = (
        lock(1, LockStatus::ChainLocked),
        lock(2, LockStatus::RecoveredFromChain),
        lock(3, LockStatus::ChainLocked),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![ready.clone(), restored.clone(), checking.clone()],
        own: Some(HashSet::from([checking.outpoint.txid])),
        ..FakeShielded::default()
    });
    lib.unknown.lock().unwrap().insert(checking.outpoint.txid);
    assert_eq!(
        state_of(&fx, &lib),
        [
            (LockOrigin::Restored, UnfinishedState::Ready),
            (LockOrigin::Restored, UnfinishedState::Checking),
            (LockOrigin::ThisDevice, UnfinishedState::Checking),
        ]
    );
    answer(&lib, Ok(Applied::Done));
    lib.queued.lock().unwrap().push_back(Ok(Applied::Done));
    let report = finish_transfers(&fx, &lib);
    assert_eq!(totals(&report), (2, 2, 1));
    assert_eq!(
        report.by_kind,
        [locks::TransfersReport {
            kind: LockKindId::Shielded,
            resumed: 2,
            completed: 2,
            still_pending: 1,
        }]
    );
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [ready.outpoint, restored.outpoint]
    );
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::ThisDevice, UnfinishedState::Checking)]
    );

    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, checking.outpoint).0.unwrap(),
        LockFinish::Applied
    );
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [ready.outpoint, restored.outpoint, checking.outpoint]
    );
}

/// Nothing unfinished, nothing reported: no kind row.
#[test]
fn finish_transfers_with_nothing_unfinished_reports_no_kind() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let report = run(&fx, &lib, move |s, lib| async move {
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[grant])
            .await)
            .unwrap();
        let mut report = super::super::FinishReport::default();
        locks::finish_kind(&ShieldLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    });
    assert_eq!(report, super::super::FinishReport::default());
}

/// Finish transfers over `lib` under a Registration lease, as Repair runs
/// it.
fn finish_transfers(fx: &Fixture, lib: &Arc<FakeShielded>) -> super::super::FinishReport {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 1));
    run(fx, lib, move |s, lib| async move {
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[grant])
            .await)
            .unwrap();
        let mut report = super::super::FinishReport::default();
        locks::finish_kind(&ShieldLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    })
}

fn totals(r: &super::super::FinishReport) -> (u32, u32, u32) {
    (r.resumed, r.completed, r.still_pending)
}

/// A lock refused before its signature (this device's here) stays pending,
/// unmarked, and the next lock still runs.
#[test]
fn finish_transfers_runs_the_next_lock_after_a_refusal() {
    let fx = Fixture::new(0);
    let (own, restored) = (
        lock(1, LockStatus::InstantSendLocked),
        lock(2, LockStatus::RecoveredFromChain),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone(), restored.clone()],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });
    answer(&lib, Err(PlatformError::Unavailable));
    lib.queued.lock().unwrap().push_back(Ok(Applied::Done));
    let report = finish_transfers(&fx, &lib);
    assert_eq!(totals(&report), (1, 1, 1));
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [own.outpoint, restored.outpoint]
    );
    assert!(
        lib.unknown.lock().unwrap().is_empty(),
        "a refusal marks nothing"
    );
    assert_eq!(*lib.settles.lock().unwrap(), [restored.outpoint.txid]);
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::ThisDevice, UnfinishedState::Ready)]
    );
}

/// Below the activation every lock is pending, none tried.
#[test]
fn finish_transfers_tries_nothing_on_an_inactive_network() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        version: 11,
        refreshed: 11,
        locks: vec![lock(1, LockStatus::RecoveredFromChain)],
        ..FakeShielded::default()
    });
    let report = finish_transfers(&fx, &lib);
    assert_eq!(totals(&report), (0, 0, 1));
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// Locks that cannot be read are not counted as "nothing pending": the
/// kind is reported unread.
#[test]
fn finish_transfers_reports_a_kind_it_could_not_read() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::RecoveredFromChain)],
        own: None,
        ..FakeShielded::default()
    });
    let report = finish_transfers(&fx, &lib);
    assert_eq!(
        report,
        super::super::FinishReport {
            unread: vec![LockKindId::Shielded],
            ..Default::default()
        }
    );
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// The session's real persister attests the atomic tracked-lock writes the
/// library's reconciliation needs, so the library records Platform's
/// "already consumed" itself. Only a backend without them refuses, with the
/// error smoke02 logged that the tests below fake. It still attests no
/// `WALLET_RESTORE`, which reconciliation no longer asks for.
#[test]
fn the_real_persister_can_record_a_consumed_report() {
    use platform_wallet::changeset::{PersistenceCapabilities, PlatformWalletPersistence};
    let fx = Fixture::new(0);
    let capabilities = fx.s.live().unwrap().store.persistence_capabilities();
    assert!(capabilities.contains(PersistenceCapabilities::ASSET_LOCK_RECONCILIATION));
    assert!(!capabilities.contains(PersistenceCapabilities::WALLET_RESTORE));
}

/// F2: a restored lock, `RecoveredFromChain` with its proof, that Platform
/// answers "already consumed" for, the library unable to record the answer
/// (a persister that does not attest reconciliation), and Platform's proved
/// state says consumed:
/// finished elsewhere, off the list and the card, settled nowhere, and so
/// after a restart, with no second apply.
#[test]
fn a_restored_lock_platform_reports_used_leaves_the_card_across_a_restart() {
    let mut fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    (lib.lib_errors.lock().unwrap()).push_back(conformance::unrecorded_consumed());
    (lib.spent_reads.lock().unwrap()).insert(restored.outpoint, SpentRead::Consumed);
    let (finished, begun) = finish(&fx, &lib, restored.outpoint);
    assert!(begun);
    assert_eq!(finished.unwrap(), LockFinish::ReportedUsed);
    assert!(listed(&fx, &lib).unwrap().is_empty(), "off the card");
    assert!(lib.settles.lock().unwrap().is_empty(), "settled nowhere");

    reopen(&mut fx, &lib);
    assert!(listed(&fx, &lib).unwrap().is_empty(), "after a restart");
    let (again, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(again.unwrap(), LockFinish::ReportedUsed);
    assert!(!begun, "no lease, so no passphrase");
    assert_eq!(*lib.applied.lock().unwrap(), [restored.outpoint]);
}

/// F2's "Finish all": both restored locks answered "already consumed",
/// unrecorded by the library and proved consumed, count as already used,
/// none pending.
#[test]
fn finish_transfers_counts_restored_locks_platform_reports_used() {
    let fx = Fixture::new(0);
    let (a, b) = (
        lock(1, LockStatus::RecoveredFromChain),
        lock(2, LockStatus::RecoveredFromChain),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![a.clone(), b.clone()],
        ..FakeShielded::default()
    });
    for at in [a.outpoint, b.outpoint] {
        (lib.lib_errors.lock().unwrap()).push_back(conformance::unrecorded_consumed());
        lib.spent_reads
            .lock()
            .unwrap()
            .insert(at, SpentRead::Consumed);
    }
    assert_eq!(
        finish_transfers(&fx, &lib),
        super::super::FinishReport {
            resumed: 2,
            completed: 0,
            still_pending: 0,
            already_used: 2,
            by_kind: vec![locks::TransfersReport {
                kind: LockKindId::Shielded,
                resumed: 2,
                completed: 0,
                still_pending: 0,
            }],
            unread: Vec::new(),
        }
    );
    assert!(listed(&fx, &lib).unwrap().is_empty());
    assert!(lib.settles.lock().unwrap().is_empty());
}

/// Only Platform's answer for the applied outpoint finishes a restored
/// lock: another persistence failure after the signature, or "already
/// consumed" naming another outpoint, leaves it Checking; this device's own
/// lock answered "already consumed" stays Checking (SR1); an untried lock
/// stays Ready.
#[test]
fn only_the_outpoints_own_answer_finishes_a_restored_lock() {
    let fx = Fixture::new(0);
    let own = lock(1, LockStatus::InstantSendLocked);
    let (failed, misnamed, untried) = (
        lock(2, LockStatus::ChainLocked),
        lock(3, LockStatus::ChainLocked),
        lock(4, LockStatus::ChainLocked),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone(), failed.clone(), misnamed.clone(), untried],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });
    let elsewhere = OutPoint::new(Txid::from_byte_array([0x99; 32]), 0);
    lib.lib_errors.lock().unwrap().extend([
        conformance::unrecorded_consumed(),
        PlatformWalletError::Persistence("database is locked".into()),
        PlatformWalletError::AssetLockAlreadyConsumed(elsewhere),
    ]);
    for at in [own.outpoint, failed.outpoint, misnamed.outpoint] {
        assert_eq!(finish(&fx, &lib, at).0.unwrap(), LockFinish::Checking);
    }
    assert_eq!(
        state_of(&fx, &lib),
        [
            (LockOrigin::ThisDevice, UnfinishedState::Checking),
            (LockOrigin::Restored, UnfinishedState::Checking),
            (LockOrigin::Restored, UnfinishedState::Checking),
            (LockOrigin::Restored, UnfinishedState::Ready),
        ]
    );
    assert!(lib.settles.lock().unwrap().is_empty());
}

/// BL-921: an earlier version kept Platform's unproved "already consumed"
/// as `recovery.shield.reported.<outpoint>`. That record no longer hides
/// the lock: it lists again, and leaves the card only once Platform's
/// proof says it is consumed (the apply signing, as a real one does).
#[test]
fn an_earlier_versions_unproved_report_no_longer_hides_a_shield_lock() {
    let rig = ShieldRig::new();
    let tx = conformance::transaction(0xB1, 1, 1_000_000, &rig.script);
    let row = conformance::tracked(
        &tx,
        0,
        AssetLockFundingType::AssetLockShieldedAddressTopUp,
        AssetLockStatus::RecoveredFromChain,
        true,
    );
    let at = row.out_point;
    rig.track(vec![row]);
    let appdb = rig.fx.s.live().unwrap().appdb;
    let scope = dw_appdb::local_scope(&rig.fx.wallet.to_string());
    let key = format!("recovery.shield.reported.{at}");
    appdb.set_setting_durable(&scope, &key, Some("1")).unwrap();
    let listed = rig.list().unwrap();
    assert_eq!(listed.len(), 1, "listed again: {listed:?}");
    assert_eq!(
        listed[0].state,
        UnfinishedState::Checking,
        "a restored lock"
    );

    rig.prove_spent(at, SpentRead::Consumed);
    rig.next(End::AlreadyConsumed);
    assert_eq!(rig.finish(at).0.unwrap(), LockFinish::ReportedUsed);
    assert!(rig.list().unwrap().is_empty(), "off the card once proved");
}
