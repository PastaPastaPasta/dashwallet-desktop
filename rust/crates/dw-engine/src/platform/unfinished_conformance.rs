//! The conformance run every [`LockKind`](locks::LockKind)
//! implementation's tests make ([`conformance`]): type 5's (`ShieldLocks`) and the runner's scripted
//! kind here; PLAT-ROUTES's type 4 runs it too. MIX-SHIELD's drain lock is a
//! `ShieldLocks` lock (funded under the shield's own steps), covered by
//! type 5's run and its drain case (`shielded_tests::unfinished`).
//! It drives the kind through the runner only ([`locks::detect`],
//! [`locks::finish`]), so it checks what a user sees: a lock settles only on
//! Platform's proved execution, an apply that signed and then failed reads
//! Checking, the refusals that can be read beforehand (among them a lock
//! below the kind's minimum, with the kind's code) ask for no lease, and
//! every record is the lock's own outpoint's.

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};

use super::errors::PlatformError;
use super::registration_flow::{LockInfo, LockStatus};
use super::unfinished_locks::{
    self as locks, KindLock, LockFinish, Marks, Minimum, UnfinishedLock, UnfinishedState,
};

/// How the kind's next apply ends, as its library would end it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// Platform's proved execution, after the lock's credit key signed.
    Done,
    /// Platform's "already consumed" for the lock (the library's typed error
    /// naming its outpoint).
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
    /// The kind's library tracks `rows` and no other lock.
    fn track(&self, rows: Vec<KindLock>);
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
    /// The kind's settled and unknown records
    /// ([`LockKind::settled_locks`](locks::LockKind::settled_locks),
    /// [`LockKind::unknown_locks`](locks::LockKind::unknown_locks)).
    fn marks(&self) -> (Marks, Marks);
    /// The kind's [`LockKind::minimum`](locks::LockKind::minimum).
    fn minimum(&self) -> Option<Minimum>;
}

/// A row of a lock transaction `n`'s credit output `vout`, of a payload of
/// `outputs` credit outputs.
pub(crate) fn row(n: u8, vout: u32, status: LockStatus, proof: bool, outputs: usize) -> KindLock {
    KindLock {
        info: LockInfo {
            outpoint: OutPoint::new(Txid::from_byte_array([n; 32]), vout),
            registration: false,
            identity_index: 0,
            account: 0,
            amount: 1_000_000,
            status,
        },
        proof,
        credit_outputs: outputs,
    }
}

fn state(rig: &impl Rig, lock: &KindLock) -> Option<UnfinishedState> {
    let at = lock.info.outpoint;
    (rig.list().unwrap().into_iter())
        .find(|l| l.txid == at.txid.to_string() && l.vout == at.vout)
        .map(|l| l.state)
}

/// Runs the contract on `rig`'s kind.
pub(crate) fn conformance(rig: &impl Rig) {
    let proved = |n, vout, outputs| row(n, vout, LockStatus::RecoveredFromChain, true, outputs);

    // Only a proved apply settles; an unproved answer, or a failure after
    // the signature however the kind returns it, reads Checking, recorded
    // by outpoint (each on a lock of its own, so no earlier record answers
    // for it).
    let ends = [End::Unknown, End::AlreadyConsumed, End::SignedThenFailed];
    for (n, end) in (0xE1..).zip(ends) {
        let lock = proved(n, 0, 1);
        rig.track(vec![lock.clone()]);
        assert_eq!(state(rig, &lock), Some(UnfinishedState::Ready), "{end:?}");
        rig.next(end);
        let (finished, asked) = rig.finish(lock.info.outpoint);
        assert_eq!(finished.unwrap(), LockFinish::Checking, "{end:?}");
        assert!(asked, "{end:?}");
        assert_eq!(
            state(rig, &lock),
            Some(UnfinishedState::Checking),
            "{end:?}"
        );
        let (settled, unknown) = rig.marks();
        assert!(!settled.holds(&lock), "{end:?} settles nothing");
        assert!(
            unknown.outpoints.contains(&lock.info.outpoint),
            "{end:?} is recorded"
        );
    }

    // A Checking lock settles on a proved apply, once.
    let a = proved(0xA1, 0, 1);
    let at = a.info.outpoint;
    rig.track(vec![a.clone()]);
    rig.next(End::Unknown);
    assert_eq!(rig.finish(at).0.unwrap(), LockFinish::Checking);
    rig.next(End::Done);
    let applies = rig.applies();
    assert_eq!(rig.finish(at).0.unwrap(), LockFinish::Applied);
    assert_eq!(rig.applies(), applies + 1);
    assert_eq!(state(rig, &a), None);
    assert!(rig.marks().0.outpoints.contains(&at), "settled by outpoint");
    let (again, asked) = rig.finish(at);
    assert_eq!(again.unwrap(), LockFinish::Applied);
    assert!(!asked, "a settled lock asks for no lease");
    assert_eq!(rig.applies(), applies + 1, "and is not applied again");

    // A refusal before the signature leaves the lock as it was.
    let d = proved(0xD1, 0, 1);
    rig.track(vec![d.clone()]);
    rig.next(End::RefusedUnsigned);
    let refused = rig.finish(d.info.outpoint).0.unwrap();
    assert!(
        matches!(refused, LockFinish::NotApplied { .. }),
        "{refused:?}"
    );
    assert_eq!(state(rig, &d), Some(UnfinishedState::Ready));
    assert!(!rig.marks().1.holds(&d));

    // What can be read beforehand asks for no lease and applies nothing:
    // a row without its proof, whatever its status says, and a credit
    // output other than the payload's first.
    let unproved = row(0xB1, 0, LockStatus::InstantSendLocked, false, 1);
    let (c0, c1) = (proved(0xC1, 0, 2), proved(0xC1, 1, 2));
    rig.track(vec![unproved.clone(), c0.clone(), c1.clone()]);
    assert_eq!(state(rig, &unproved), Some(UnfinishedState::Confirming));
    assert_eq!(state(rig, &c0), Some(UnfinishedState::Ready));
    assert_eq!(state(rig, &c1), Some(UnfinishedState::Unsupported));
    let applies = rig.applies();
    let (waiting, asked) = rig.finish(unproved.info.outpoint);
    assert_eq!(waiting.unwrap(), LockFinish::ProofWaiting);
    assert!(!asked);
    let (unsupported, asked) = rig.finish(c1.info.outpoint);
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
    assert_eq!(rig.finish(c0.info.outpoint).0.unwrap(), LockFinish::Applied);
    assert_eq!(state(rig, &c0), None);
    assert_eq!(state(rig, &c1), Some(UnfinishedState::Unsupported));
    let (settled, _) = rig.marks();
    assert!(settled.outpoints.contains(&c0.info.outpoint));
    assert!(!settled.outpoints.contains(&c1.info.outpoint));
    assert!(
        !settled.txids.contains(&c0.info.outpoint.txid),
        "no record by txid for a transaction of several locks"
    );

    // A lock below the kind's minimum lists apart and is refused with the
    // kind's code before the lease; one at the minimum is Ready.
    let min = rig.minimum().expect("the kind's minimum reads");
    let worth = |n, duffs| {
        let mut lock = proved(n, 0, 1);
        lock.info.amount = duffs;
        lock
    };
    let (small, at_min) = (worth(0xF1, min.duffs - 1), worth(0xF2, min.duffs));
    rig.track(vec![small.clone(), at_min.clone()]);
    assert_eq!(state(rig, &small), Some(UnfinishedState::TooSmall));
    assert_eq!(state(rig, &at_min), Some(UnfinishedState::Ready));
    let applies = rig.applies();
    let (refused, asked) = rig.finish(small.info.outpoint);
    assert_eq!(
        refused.unwrap(),
        LockFinish::NotApplied {
            error: min.code.into()
        }
    );
    assert!(!asked, "a lock too small asks for no lease");
    assert_eq!(rig.applies(), applies);
}
