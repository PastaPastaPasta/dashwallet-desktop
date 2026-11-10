//! The conformance run every [`LockKind`](locks::LockKind)
//! implementation's tests make ([`conformance`]): type 5's (`ShieldLocks`) and the runner's scripted
//! kind here; PLAT-ROUTES's type 4 runs it too. MIX-SHIELD's drain lock is a
//! `ShieldLocks` lock (funded under the shield's own steps), covered by
//! type 5's run and its drain case (`shielded_tests::unfinished`).
//! It drives the kind through the runner only ([`locks::detect`],
//! [`locks::finish`]), over the library's own tracked rows
//! ([`TrackedAssetLock`], with their transactions), so it checks what a user
//! sees and how the kind builds its rows: a lock settles only on Platform's
//! proved execution, an apply that signed and then failed reads Checking
//! and ends its call `MaybeSent`, the refusals that can be read beforehand
//! (among them a lock below the kind's minimum, with the kind's code) ask
//! for no lease, and every record is the lock's own outpoint's, so no txid
//! record of another output of its transaction hides it.

use dashcore::hashes::Hash;
use dashcore::transaction::special_transaction::TransactionPayload;
use dashcore::transaction::special_transaction::asset_lock::AssetLockPayload;
use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::prelude::AssetLockProof;
use platform_wallet::{AssetLockFundingType, AssetLockStatus, TrackedAssetLock};

use super::errors::PlatformError;
use super::lease::{FlowOutcome, Lease};
use super::unfinished_locks::{
    self as locks, KindLock, LockFinish, Mark, Marks, Minimum, UnfinishedLock, UnfinishedState,
};

/// How the kind's next apply ends, as its library would end it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// Platform's proved execution, after the lock's credit key signed.
    Done,
    /// Platform's "already consumed" for the lock (the library's typed error
    /// naming its outpoint), after the signature.
    AlreadyConsumed,
    /// A failure after the signature the kind reports as unknown.
    Unknown,
    /// A refusal before any signature.
    RefusedUnsigned,
    /// A failure after the lock's credit key signed, returned as an error.
    SignedThenFailed,
}

/// A kind under test, with its library scripted.
pub(crate) trait Rig {
    /// The funding type of the kind's locks.
    fn funding_type(&self) -> AssetLockFundingType;
    /// The script of the credit output the kind's lock key pays (what its
    /// narrowed signer accepts).
    fn credit_script(&self) -> ScriptBuf;
    /// The kind's library tracks `rows` as well (earlier rows may stay): the
    /// rows its `locks()` reads and builds its [`KindLock`]s from.
    fn track(&self, rows: Vec<TrackedAssetLock>);
    /// Records `txid` as the kind's 0.1 records by txid do (what a settle or
    /// an unknown result of a lock alone in its transaction writes), for
    /// whatever output of it.
    fn record_txid(&self, mark: Mark, txid: Txid);
    /// The next apply ends as `end`, signing through the signer it is
    /// handed (with the lock's credit key) first when `end` comes after a
    /// signature.
    fn next(&self, end: End);
    /// The applies the kind's library ran.
    fn applies(&self) -> usize;
    /// [`locks::detect`] over the kind, nothing reported used.
    fn list(&self) -> Result<Vec<UnfinishedLock>, PlatformError>;
    /// [`locks::finish`] of `outpoint` over the kind, its lease of a fresh
    /// grant; and whether that lease was asked for.
    fn finish(&self, outpoint: OutPoint) -> (Result<LockFinish, PlatformError>, bool);
    /// The outcomes of the calls the last [`Self::finish`]'s lease recorded
    /// (`CallPermit::finish`), read before the lease ended; none when no
    /// lease was asked for.
    fn outcomes(&self) -> Vec<FlowOutcome>;
    /// The kind's settled and unknown records
    /// ([`LockKind::settled_locks`](locks::LockKind::settled_locks),
    /// [`LockKind::unknown_locks`](locks::LockKind::unknown_locks)).
    fn marks(&self) -> (Marks, Marks);
    /// The kind's [`LockKind::minimum`](locks::LockKind::minimum).
    fn minimum(&self) -> Option<Minimum>;
    /// Whether the kind takes Platform's "already consumed" as finished
    /// elsewhere ([`LockKind::reported_locks`](locks::LockKind::reported_locks)).
    fn reports_used(&self) -> bool {
        false
    }
}

/// A lock transaction `n` of `outputs` credit outputs of `duffs` each, every
/// one paying `script`.
pub(crate) fn transaction(n: u8, outputs: usize, duffs: u64, script: &ScriptBuf) -> Transaction {
    Transaction {
        version: 3,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload {
                version: 1,
                credit_outputs: (0..outputs)
                    .map(|_| TxOut {
                        value: duffs,
                        script_pubkey: script.clone(),
                    })
                    .collect(),
            },
        )),
    }
}

/// The library's row of `transaction`'s credit output `vout`, of
/// `funding_type`, with its ChainLock proof when `proof`.
pub(crate) fn tracked(
    transaction: &Transaction,
    vout: u32,
    funding_type: AssetLockFundingType,
    status: AssetLockStatus,
    proof: bool,
) -> TrackedAssetLock {
    let out_point = OutPoint::new(transaction.txid(), vout);
    let amount = match &transaction.special_transaction_payload {
        Some(TransactionPayload::AssetLockPayloadType(p)) => p.credit_outputs[vout as usize].value,
        _ => 0,
    };
    TrackedAssetLock {
        out_point,
        proof: proof
            .then(|| AssetLockProof::Chain(ChainAssetLockProof::new(100, out_point.into()))),
        transaction: transaction.clone(),
        account_index: 0,
        funding_type,
        identity_index: 0,
        amount,
        status,
    }
}

/// The outcomes of the calls `lease` recorded so far (for a rig's
/// [`Rig::outcomes`], with a clone of the finish's lease kept until it read
/// them).
pub(crate) fn call_outcomes(lease: &Lease) -> Vec<FlowOutcome> {
    lease.table.inspect(|i| {
        (i.leases.get(&lease.id))
            .map(|e| e.calls_done.iter().map(|c| c.outcome).collect())
            .unwrap_or_default()
    })
}

fn state(rig: &impl Rig, at: OutPoint) -> Option<UnfinishedState> {
    (rig.list().unwrap().into_iter())
        .find(|l| l.txid == at.txid.to_string() && l.vout == at.vout)
        .map(|l| l.state)
}

/// A funding type other than `kind`'s.
fn other_family(kind: AssetLockFundingType) -> AssetLockFundingType {
    match kind {
        AssetLockFundingType::IdentityTopUp => AssetLockFundingType::AssetLockAddressTopUp,
        _ => AssetLockFundingType::IdentityTopUp,
    }
}

/// Runs the contract on `rig`'s kind.
pub(crate) fn conformance(rig: &impl Rig) {
    let (kind, script) = (rig.funding_type(), rig.credit_script());
    let proved_tx = |n, outputs| transaction(n, outputs, 1_000_000, &script);
    let proved =
        |tx: &Transaction, vout| tracked(tx, vout, kind, AssetLockStatus::RecoveredFromChain, true);
    let row = KindLock::of_row;

    // Only a proved apply settles, and only its call ends `Sent`; an
    // unproved answer, or a failure after the signature however the kind
    // returns it, reads Checking, recorded by outpoint, its call
    // `MaybeSent` (each on a lock of its own, so no earlier record answers
    // for it). A kind that takes "already consumed" as finished elsewhere
    // reads it as reported used instead: the lock leaves the list, settled
    // nowhere.
    let ends = [End::Unknown, End::AlreadyConsumed, End::SignedThenFailed];
    for (n, end) in (0xE1..).zip(ends) {
        let lock = proved(&proved_tx(n, 1), 0);
        let at = lock.out_point;
        rig.track(vec![lock.clone()]);
        assert_eq!(state(rig, at), Some(UnfinishedState::Ready), "{end:?}");
        rig.next(end);
        let (finished, asked) = rig.finish(at);
        assert!(asked, "{end:?}");
        assert_eq!(rig.outcomes(), [FlowOutcome::MaybeSent], "{end:?}");
        let (settled, unknown) = rig.marks();
        assert!(!settled.holds(&row(&lock)), "{end:?} settles nothing");
        if end == End::AlreadyConsumed && rig.reports_used() {
            assert_eq!(finished.unwrap(), LockFinish::ReportedUsed);
            assert_eq!(state(rig, at), None, "reported used leaves the list");
            continue;
        }
        assert_eq!(finished.unwrap(), LockFinish::Checking, "{end:?}");
        assert_eq!(state(rig, at), Some(UnfinishedState::Checking), "{end:?}");
        assert!(unknown.outpoints.contains(&at), "{end:?} is recorded");
    }

    // A Checking lock settles on a proved apply, once.
    let a = proved(&proved_tx(0xA1, 1), 0);
    let at = a.out_point;
    rig.track(vec![a.clone()]);
    rig.next(End::Unknown);
    assert_eq!(rig.finish(at).0.unwrap(), LockFinish::Checking);
    rig.next(End::Done);
    let applies = rig.applies();
    assert_eq!(rig.finish(at).0.unwrap(), LockFinish::Applied);
    assert_eq!(rig.outcomes(), [FlowOutcome::Sent]);
    assert_eq!(rig.applies(), applies + 1);
    assert_eq!(state(rig, at), None);
    assert!(rig.marks().0.outpoints.contains(&at), "settled by outpoint");
    let (again, asked) = rig.finish(at);
    assert_eq!(again.unwrap(), LockFinish::Applied);
    assert!(!asked, "a settled lock asks for no lease");
    assert!(rig.outcomes().is_empty());
    assert_eq!(rig.applies(), applies + 1, "and is not applied again");

    // A refusal before the signature leaves the lock as it was, its call
    // `Cancelled`.
    let d = proved(&proved_tx(0xD1, 1), 0);
    rig.track(vec![d.clone()]);
    rig.next(End::RefusedUnsigned);
    let refused = rig.finish(d.out_point).0.unwrap();
    assert!(
        matches!(refused, LockFinish::NotApplied { .. }),
        "{refused:?}"
    );
    assert_eq!(rig.outcomes(), [FlowOutcome::Cancelled]);
    assert_eq!(state(rig, d.out_point), Some(UnfinishedState::Ready));
    assert!(!rig.marks().1.holds(&row(&d)));

    // What can be read beforehand asks for no lease and applies nothing:
    // a row without its proof, whatever its status says, and a credit
    // output other than the payload's first.
    let unproved = tracked(
        &proved_tx(0xB1, 1),
        0,
        kind,
        AssetLockStatus::InstantSendLocked,
        false,
    );
    let c = proved_tx(0xC1, 2);
    let (c0, c1) = (proved(&c, 0), proved(&c, 1));
    rig.track(vec![unproved.clone(), c0.clone(), c1.clone()]);
    assert_eq!(
        state(rig, unproved.out_point),
        Some(UnfinishedState::Confirming)
    );
    assert_eq!(state(rig, c0.out_point), Some(UnfinishedState::Ready));
    assert_eq!(state(rig, c1.out_point), Some(UnfinishedState::Unsupported));
    let applies = rig.applies();
    let (waiting, asked) = rig.finish(unproved.out_point);
    assert_eq!(waiting.unwrap(), LockFinish::ProofWaiting);
    assert!(!asked);
    let (unsupported, asked) = rig.finish(c1.out_point);
    assert_eq!(
        unsupported.unwrap(),
        LockFinish::NotApplied {
            error: locks::UNSUPPORTED.into()
        }
    );
    assert!(!asked);
    assert_eq!(rig.applies(), applies);

    // Two credit outputs of one transaction are two locks: settling the
    // first records its outpoint only, so the second stays listed.
    rig.next(End::Done);
    assert_eq!(rig.finish(c0.out_point).0.unwrap(), LockFinish::Applied);
    assert_eq!(state(rig, c0.out_point), None);
    assert_eq!(state(rig, c1.out_point), Some(UnfinishedState::Unsupported));
    let (settled, _) = rig.marks();
    assert!(settled.outpoints.contains(&c0.out_point));
    assert!(!settled.outpoints.contains(&c1.out_point));
    assert!(
        !settled.txids.contains(&c.txid()),
        "no record by txid for a transaction of several locks"
    );

    // The kind builds its rows from the whole payload: its output 0 shares
    // its transaction with another family's output 1, whose settle and
    // unknown result were recorded by txid, so neither record speaks for
    // it. It lists Ready and its finish reaches the lease and the apply.
    let shared = proved_tx(0x51, 2);
    let (own, sibling) = (
        proved(&shared, 0),
        tracked(
            &shared,
            1,
            other_family(kind),
            AssetLockStatus::RecoveredFromChain,
            true,
        ),
    );
    rig.track(vec![own.clone(), sibling]);
    rig.record_txid(Mark::Settled, shared.txid());
    rig.record_txid(Mark::Unknown, shared.txid());
    assert_eq!(state(rig, own.out_point), Some(UnfinishedState::Ready));
    rig.next(End::Done);
    let applies = rig.applies();
    let (finished, asked) = rig.finish(own.out_point);
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert!(asked, "a txid record of the transaction finishes nothing");
    assert_eq!(rig.applies(), applies + 1);

    // A lock below the kind's minimum lists apart and is refused with the
    // kind's code before the lease; one at the minimum is Ready.
    let min = rig.minimum().expect("the kind's minimum reads");
    let worth = |n, duffs| proved(&transaction(n, 1, duffs, &script), 0);
    let (small, at_min) = (worth(0xF1, min.duffs - 1), worth(0xF2, min.duffs));
    rig.track(vec![small.clone(), at_min.clone()]);
    assert_eq!(state(rig, small.out_point), Some(UnfinishedState::TooSmall));
    assert_eq!(state(rig, at_min.out_point), Some(UnfinishedState::Ready));
    let applies = rig.applies();
    let (refused, asked) = rig.finish(small.out_point);
    assert_eq!(
        refused.unwrap(),
        LockFinish::NotApplied {
            error: min.code.into()
        }
    );
    assert!(!asked, "a lock too small asks for no lease");
    assert_eq!(rig.applies(), applies);
}
