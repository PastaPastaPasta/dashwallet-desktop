//! Tests of unfinished_locks.rs: a lock's standing from the library's row
//! and the engine's evidence, the one reply that reads "already consumed",
//! and detection over a scripted kind.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use key_wallet::bip32::DerivationPath;
use platform_wallet::{AssetLockFundingType, PlatformWalletError};

use super::errors::PlatformError;
use super::flows::FlowKind;
use super::lease::Lease;
use super::registration_flow::{LockInfo, LockStatus};
use super::unfinished_conformance::{self as conformance, End, Rig};
use super::unfinished_locks::{
    ApplyReply, Evidence, KindLock, LockFinish, LockKeySigner, LockKind, LockKindId, LockOrigin,
    Marks, Minimum, Readiness, UNSUPPORTED, UnfinishedLock, UnfinishedState, classify, detect,
    finish, reply_from,
};

fn lock(n: u8, status: LockStatus) -> LockInfo {
    LockInfo {
        outpoint: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
        registration: false,
        identity_index: 0,
        account: 0,
        amount: 1_000_000 * u64::from(n),
        status,
    }
}

/// `l` as the library tracks it: its proof as its status says, alone in
/// its transaction.
fn alone(l: &LockInfo) -> KindLock {
    KindLock {
        info: l.clone(),
        proof: l.status.has_proof(),
        credit_outputs: 1,
    }
}

fn rows(locks: &[LockInfo]) -> Mutex<Vec<KindLock>> {
    Mutex::new(locks.iter().map(alone).collect())
}

/// 0.1's records of `txids`.
fn txids<const N: usize>(txids: [Txid; N]) -> Marks {
    Marks {
        txids: HashSet::from(txids),
        ..Marks::default()
    }
}

/// The evidence, empty but for what `f` adds.
struct Ev {
    own: HashSet<Txid>,
    settled: Marks,
    unknown: Marks,
    used: HashSet<OutPoint>,
    readiness: Readiness,
    minimum: Option<Minimum>,
}

impl Default for Ev {
    fn default() -> Self {
        Self {
            own: HashSet::new(),
            settled: Marks::default(),
            unknown: Marks::default(),
            used: HashSet::new(),
            readiness: Readiness::Ready,
            minimum: None,
        }
    }
}

impl Ev {
    fn state(&self, l: &LockInfo) -> Option<(LockOrigin, UnfinishedState)> {
        self.state_of(&alone(l))
    }

    fn state_of(&self, l: &KindLock) -> Option<(LockOrigin, UnfinishedState)> {
        let ev = Evidence {
            own: &self.own,
            settled: &self.settled,
            unknown: &self.unknown,
            reported_used: &self.used,
            readiness: &self.readiness,
            minimum: self.minimum,
        };
        classify(LockKindId::Shielded, l, &ev).map(|u| (u.origin, u.state))
    }
}

#[test]
fn every_kind_names_its_funding_type_and_no_other_type_is_a_kind() {
    use AssetLockFundingType as T;
    for k in LockKindId::ALL {
        assert_eq!(LockKindId::of(k.funding_type()), Some(k));
    }
    assert_eq!(
        LockKindId::of(T::AssetLockShieldedAddressTopUp),
        Some(LockKindId::Shielded)
    );
    assert_eq!(
        LockKindId::of(T::AssetLockAddressTopUp),
        Some(LockKindId::PlatformAddress)
    );
    for t in [
        T::IdentityRegistration,
        T::IdentityTopUp,
        T::IdentityTopUpNotBound,
        T::IdentityInvitation,
    ] {
        assert_eq!(LockKindId::of(t), None, "{t:?} is no lock kind");
    }
}

/// A lock a restore read back from the chain that no funding step here
/// signed is unfinished and ready to try: nothing tells whether Platform
/// took it.
#[test]
fn a_restored_chain_locked_lock_is_ready() {
    let l = lock(1, LockStatus::RecoveredFromChain);
    let state = Ev::default().state(&l);
    assert_eq!(state, Some((LockOrigin::Restored, UnfinishedState::Ready)));
}

/// A lock of this device's the library holds "consumption unknown" reads
/// Checking, as `pending_shields` lists it (SR1).
#[test]
fn an_own_recovered_lock_is_checking() {
    let l = lock(1, LockStatus::RecoveredFromChain);
    let ev = Ev {
        own: HashSet::from([l.outpoint.txid]),
        ..Ev::default()
    };
    assert_eq!(
        ev.state(&l),
        Some((LockOrigin::ThisDevice, UnfinishedState::Checking))
    );
    let proved = lock(2, LockStatus::InstantSendLocked);
    let ev = Ev {
        own: HashSet::from([proved.outpoint.txid]),
        ..Ev::default()
    };
    assert_eq!(
        ev.state(&proved),
        Some((LockOrigin::ThisDevice, UnfinishedState::Ready))
    );
}

#[test]
fn a_consumed_or_settled_lock_is_finished() {
    let consumed = lock(1, LockStatus::Consumed);
    assert_eq!(Ev::default().state(&consumed), None);
    let settled = lock(2, LockStatus::RecoveredFromChain);
    let ev = Ev {
        settled: txids([settled.outpoint.txid]),
        ..Ev::default()
    };
    assert_eq!(ev.state(&settled), None);
}

/// A lock without a proof has nothing to apply yet, whatever else is
/// known of it.
#[test]
fn a_lock_without_a_proof_is_confirming() {
    for status in [LockStatus::Built, LockStatus::Broadcast] {
        let l = lock(1, status);
        let ev = Ev {
            unknown: txids([l.outpoint.txid]),
            used: HashSet::from([l.outpoint]),
            readiness: Readiness::NotActive("shielded.not_active".into()),
            ..Ev::default()
        };
        assert_eq!(
            ev.state(&l),
            Some((LockOrigin::Restored, UnfinishedState::Confirming)),
            "{status:?}"
        );
    }
}

#[test]
fn an_apply_that_may_have_executed_is_checking() {
    let l = lock(1, LockStatus::RecoveredFromChain);
    let ev = Ev {
        unknown: txids([l.outpoint.txid]),
        ..Ev::default()
    };
    assert_eq!(
        ev.state(&l),
        Some((LockOrigin::Restored, UnfinishedState::Checking))
    );
}

/// Only a restored lock reads "reported used", and only for its own
/// outpoint; a lock of this device's keeps SR1's Checking.
#[test]
fn reported_used_is_for_the_restored_lock_of_that_outpoint() {
    let l = lock(1, LockStatus::RecoveredFromChain);
    let ev = Ev {
        used: HashSet::from([l.outpoint]),
        ..Ev::default()
    };
    assert_eq!(
        ev.state(&l),
        Some((LockOrigin::Restored, UnfinishedState::ReportedUsed))
    );
    let other_vout = Ev {
        used: HashSet::from([OutPoint::new(l.outpoint.txid, 1)]),
        ..Ev::default()
    };
    assert_eq!(
        other_vout.state(&l),
        Some((LockOrigin::Restored, UnfinishedState::Ready))
    );
    let own = Ev {
        own: HashSet::from([l.outpoint.txid]),
        used: HashSet::from([l.outpoint]),
        ..Ev::default()
    };
    assert_eq!(
        own.state(&l),
        Some((LockOrigin::ThisDevice, UnfinishedState::Checking))
    );
}

#[test]
fn a_proved_lock_of_an_inactive_kind_is_not_active() {
    let l = lock(1, LockStatus::ChainLocked);
    let ev = Ev {
        readiness: Readiness::NotActive("shielded.not_active".into()),
        ..Ev::default()
    };
    assert_eq!(
        ev.state(&l),
        Some((LockOrigin::Restored, UnfinishedState::NotActive))
    );
    let held = Ev {
        readiness: Readiness::Held("platform.unavailable".into()),
        ..Ev::default()
    };
    assert_eq!(
        held.state(&l),
        Some((LockOrigin::Restored, UnfinishedState::Ready)),
        "a hold that clears by itself changes no standing"
    );
}

/// The one answer that reads "already consumed" is the library's typed
/// error naming the applied outpoint.
#[test]
fn only_already_consumed_of_the_same_outpoint_reads_already_consumed() {
    let at = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let consumed = PlatformWalletError::AssetLockAlreadyConsumed;
    assert_eq!(reply_from(Ok(()), at, true).unwrap(), ApplyReply::Done);
    for signed in [false, true] {
        assert_eq!(
            reply_from(Err(consumed(at)), at, signed).unwrap(),
            ApplyReply::AlreadyConsumed
        );
    }
    for other in [
        OutPoint::new(at.txid, 1),
        OutPoint::new(Txid::from_byte_array([2; 32]), 0),
    ] {
        assert_eq!(
            reply_from(Err(consumed(other)), at, true).unwrap(),
            ApplyReply::Unknown
        );
        assert!(reply_from(Err(consumed(other)), at, false).is_err());
    }
}

/// A timeout, a broadcast failure or any other error is never "already
/// consumed": after the signature it may have executed, before it it is
/// the refusal.
#[test]
fn no_other_error_or_timeout_reads_already_consumed() {
    let at = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let others = || {
        [
            PlatformWalletError::FinalityTimeout(at),
            PlatformWalletError::TransactionBroadcast("timed out".into()),
            PlatformWalletError::AssetLockNotTracked(at),
            PlatformWalletError::AssetLockProofWait("no proof".into()),
        ]
    };
    for e in others() {
        let shown = format!("{e:?}");
        assert_eq!(
            reply_from(Err(e), at, true).unwrap(),
            ApplyReply::Unknown,
            "{shown}"
        );
    }
    for e in others() {
        let shown = format!("{e:?}");
        assert!(reply_from(Err(e), at, false).is_err(), "{shown}");
    }
}

/// How the scripted apply ends (`FakeKind::ends`, else [`End::Done`]).
type Ends = Mutex<VecDeque<End>>;

/// platform-wallet's tracked rows, scripted.
#[derive(Default)]
struct FakeKind {
    locks: Mutex<Vec<KindLock>>,
    own: Option<HashSet<Txid>>,
    settled: Mutex<Marks>,
    unknown: Mutex<Marks>,
    /// What holds an apply back (`Readiness::Held`).
    held: Option<String>,
    /// [`LockKind::minimum`].
    minimum: Option<Minimum>,
    /// The tracked credit script of every lock.
    script: Option<dashcore::ScriptBuf>,
    /// The keys an apply asks its signer for, and whether each signed.
    signs: Vec<DerivationPath>,
    signed: Mutex<Vec<bool>>,
    ends: Ends,
    applies: AtomicU32,
}

impl LockKind for FakeKind {
    fn id(&self) -> LockKindId {
        LockKindId::Shielded
    }

    fn flow(&self) -> FlowKind {
        FlowKind::Shield
    }

    async fn locks(&self) -> Vec<KindLock> {
        self.locks.lock().unwrap().clone()
    }

    fn own_locks(&self) -> Option<HashSet<Txid>> {
        self.own.clone()
    }

    async fn settled_locks(&self) -> Marks {
        self.settled.lock().unwrap().clone()
    }

    async fn unknown_locks(&self) -> Marks {
        self.unknown.lock().unwrap().clone()
    }

    async fn mark_unknown(&self, lock: &KindLock) {
        (self.unknown.lock().unwrap().outpoints).insert(lock.info.outpoint);
    }

    async fn readiness(&self) -> Readiness {
        self.held.clone().map_or(Readiness::Ready, Readiness::Held)
    }

    fn not_active(&self) -> Option<String> {
        None
    }

    fn minimum(&self) -> Option<Minimum> {
        self.minimum
    }

    fn caught_up(&self) -> bool {
        true
    }

    fn wallet_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::default()
    }

    async fn credit_script(&self, _: OutPoint) -> Option<dashcore::ScriptBuf> {
        self.script.clone()
    }

    /// Signs as the library's transition does, with every key of `signs`
    /// (none for a refusal), then ends as scripted. A failure after the
    /// signature comes back as the error, as a careless kind would return
    /// it.
    async fn apply(
        &self,
        _: OutPoint,
        signer: &LockKeySigner<'_>,
    ) -> Result<ApplyReply, PlatformError> {
        use key_wallet::Signer as _;
        self.applies.fetch_add(1, Ordering::SeqCst);
        let end = self.ends.lock().unwrap().pop_front().unwrap_or(End::Done);
        if end == End::RefusedUnsigned {
            return Err(PlatformError::Unavailable);
        }
        for path in &self.signs {
            let signed = signer.sign_ecdsa(path, [7; 32]).await.is_ok();
            self.signed.lock().unwrap().push(signed);
        }
        match end {
            End::Done => Ok(ApplyReply::Done),
            End::AlreadyConsumed => Ok(ApplyReply::AlreadyConsumed),
            End::Unknown => Ok(ApplyReply::Unknown),
            End::SignedThenFailed => Err(PlatformError::Internal {
                detail: "broadcast failed".into(),
            }),
            End::RefusedUnsigned => unreachable!(),
        }
    }

    async fn settle(&self, lock: &KindLock) {
        (self.settled.lock().unwrap().outpoints).insert(lock.info.outpoint);
    }
}

fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

/// Until the journal is read nothing can be told apart: no list.
#[test]
fn detection_waits_for_the_journal() {
    let kind = FakeKind {
        locks: rows(&[lock(1, LockStatus::RecoveredFromChain)]),
        own: None,
        ..FakeKind::default()
    };
    let listed = block_on(detect(&kind, &HashSet::new()));
    assert_eq!(listed.unwrap_err().code(), "platform.unavailable");
}

#[test]
fn detection_lists_the_unfinished_locks_only_sorted() {
    let (own, restored, done) = (
        lock(3, LockStatus::InstantSendLocked),
        lock(2, LockStatus::RecoveredFromChain),
        lock(1, LockStatus::Consumed),
    );
    let kind = FakeKind {
        locks: rows(&[own.clone(), restored.clone(), done]),
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeKind::default()
    };
    let listed = block_on(detect(&kind, &HashSet::new())).unwrap();
    let got: Vec<_> = (listed.iter())
        .map(|u| (u.txid.clone(), u.duffs, u.origin, u.state))
        .collect();
    assert_eq!(
        got,
        [
            (
                restored.outpoint.txid.to_string(),
                2_000_000,
                LockOrigin::Restored,
                UnfinishedState::Ready
            ),
            (
                own.outpoint.txid.to_string(),
                3_000_000,
                LockOrigin::ThisDevice,
                UnfinishedState::Ready
            ),
        ]
    );
}

/// Q2: a finish signs with the lock's credit key and no other. The lease's
/// funding signer (`PlatformFunding`) would sign a Core address key too;
/// narrowed to the lock it refuses that, and another lock's credit key,
/// before the vault is asked.
#[test]
fn a_finish_signs_with_the_lock_key_only() {
    use std::str::FromStr;

    use key_wallet::Signer as _;

    let fx = super::requests::tests::Fixture::new(0);
    let grant = (fx.s.vault)
        .authorize(
            dw_vault::GrantPurpose::PlatformOp {
                max_duffs: 1,
                max_credits: 0,
            },
            Some(&fx.wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    fx.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant]).await).unwrap();
        let funding = lease.uncounted_funding_signer().unwrap();
        let path = |p: &str| DerivationPath::from_str(p).unwrap();
        let (lock_key, other_lock, core) = (
            path("m/9'/1'/5'/5'/0"),
            path("m/9'/1'/5'/5'/1"),
            path("m/44'/1'/0'/0/0"),
        );
        // The scope alone signs a Core address key.
        assert!(funding.sign_ecdsa(&core, [7; 32]).await.is_ok());

        let key = funding.public_key(&lock_key).await.unwrap();
        let script = dashcore::ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash());
        let count = Arc::new(super::lease::SignatureCount::default());
        let narrowed = LockKeySigner::new(&funding, script, Arc::clone(&count));
        for refused in [&core, &other_lock] {
            let e = narrowed.sign_ecdsa(refused, [7; 32]).await.unwrap_err();
            assert!(matches!(e, dw_vault::SignerError::KeyMismatch(_)), "{e:?}");
        }
        assert!(!narrowed.signed(), "a refusal is no signature");
        let (_, signed_with) = narrowed.sign_ecdsa(&lock_key, [7; 32]).await.unwrap();
        assert_eq!(signed_with, key);
        assert!(narrowed.signed());
        assert_eq!(count.released(), 1, "counted into the permit");
        count.close();
        let e = narrowed.sign_ecdsa(&lock_key, [7; 32]).await.unwrap_err();
        assert!(
            matches!(e, dw_vault::SignerError::Locked),
            "the permit ended: {e:?}"
        );
    });
}

/// A shield's apply reads Platform's "already consumed" as `Taken` only
/// when it names the applied lock: one naming another outpoint is an
/// ordinary failure (after the signature: unknown, before it: the
/// refusal).
#[test]
fn a_shield_apply_reads_already_consumed_only_for_its_own_lock() {
    use super::shielded_pool::{Applied, applied_from};
    let at = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let other = OutPoint::new(Txid::from_byte_array([2; 32]), 0);
    let consumed = PlatformWalletError::AssetLockAlreadyConsumed;
    assert_eq!(
        applied_from(Err(consumed(at)), at, true).unwrap(),
        Applied::Taken
    );
    assert_eq!(
        applied_from(Err(consumed(other)), at, true).unwrap(),
        Applied::Unknown
    );
    assert!(applied_from(Err(consumed(other)), at, false).is_err());
}

/// A lock transaction with two credit outputs is two locks of one txid.
/// 0.1's records by txid (settled, unknown) say nothing of either; the
/// runner's, by outpoint, speak for theirs only. Its first output is
/// finishable, its second can't be in this version.
#[test]
fn a_txid_of_two_locks_is_told_apart_by_outpoint() {
    let first = KindLock {
        credit_outputs: 2,
        ..alone(&lock(1, LockStatus::ChainLocked))
    };
    let second = KindLock {
        info: LockInfo {
            outpoint: OutPoint::new(first.info.outpoint.txid, 1),
            ..first.info.clone()
        },
        ..first.clone()
    };
    let txid = first.info.outpoint.txid;
    let by_txid = Ev {
        settled: txids([txid]),
        unknown: txids([txid]),
        own: HashSet::from([txid]),
        ..Ev::default()
    };
    assert_eq!(
        by_txid.state_of(&first),
        Some((LockOrigin::Restored, UnfinishedState::Ready))
    );
    assert_eq!(
        by_txid.state_of(&second),
        Some((LockOrigin::Restored, UnfinishedState::Unsupported))
    );
    let of_first = |at: OutPoint| Marks {
        outpoints: HashSet::from([at]),
        ..Marks::default()
    };
    let settled = Ev {
        settled: of_first(first.info.outpoint),
        ..Ev::default()
    };
    assert_eq!(settled.state_of(&first), None);
    assert_eq!(
        settled.state_of(&second),
        Some((LockOrigin::Restored, UnfinishedState::Unsupported))
    );
    let unknown = Ev {
        unknown: of_first(first.info.outpoint),
        ..Ev::default()
    };
    assert_eq!(
        unknown.state_of(&first),
        Some((LockOrigin::Restored, UnfinishedState::Checking))
    );
    let consumed = KindLock {
        info: LockInfo {
            status: LockStatus::Consumed,
            ..first.info.clone()
        },
        ..first.clone()
    };
    assert_eq!(Ev::default().state_of(&consumed), None, "the library's row");

    let kind = FakeKind {
        locks: Mutex::new(vec![second, first]),
        own: Some(HashSet::new()),
        settled: Mutex::new(txids([txid])),
        ..FakeKind::default()
    };
    let listed = block_on(detect(&kind, &HashSet::new())).unwrap();
    let vouts: Vec<_> = listed.iter().map(|u| (u.vout, u.state)).collect();
    assert_eq!(
        vouts,
        [
            (0, UnfinishedState::Ready),
            (1, UnfinishedState::Unsupported)
        ]
    );
}

/// The case 0.1's txid records got wrong (Sol M1): one transaction paying
/// a top-up credit key and a shield credit key. The kind tracks only its
/// own output, but the payload's count says the txid is shared, so the
/// top-up's settled record does not hide the unconsumed shield output, and
/// a finish applies it rather than answering "applied".
#[test]
fn a_sibling_of_another_kind_settled_by_txid_hides_nothing() {
    let shield = KindLock {
        credit_outputs: 2,
        ..alone(&lock(1, LockStatus::RecoveredFromChain))
    };
    let txid = shield.info.outpoint.txid;
    let kind = FakeKind {
        locks: Mutex::new(vec![shield.clone()]),
        own: Some(HashSet::new()),
        settled: Mutex::new(txids([txid])),
        unknown: Mutex::new(txids([txid])),
        script: Some(dashcore::ScriptBuf::new()),
        ..FakeKind::default()
    };
    let listed = block_on(detect(&kind, &HashSet::new())).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, UnfinishedState::Ready);

    let asked = AtomicBool::new(false);
    let lease = || async {
        asked.store(true, Ordering::SeqCst);
        Err::<Lease, _>(PlatformError::Unavailable)
    };
    let finished = block_on(finish(&kind, shield.info.outpoint, lease));
    assert_eq!(finished.unwrap_err(), PlatformError::Unavailable);
    assert!(
        asked.load(Ordering::SeqCst),
        "it goes on to apply, not to \"applied\""
    );
}

/// Sol M2: the pinned library re-derives the key of a lock's first credit
/// output whatever its outpoint, so an output at `vout` 1 lists as not
/// finishable in this version, never Ready, and a finish of it refuses
/// before the lease; output 0 of the same transaction still finishes.
#[test]
fn a_lock_at_another_output_cannot_be_finished_in_this_version() {
    let first = KindLock {
        credit_outputs: 2,
        ..alone(&lock(1, LockStatus::RecoveredFromChain))
    };
    let second = KindLock {
        info: LockInfo {
            outpoint: OutPoint::new(first.info.outpoint.txid, 1),
            ..first.info.clone()
        },
        ..first.clone()
    };
    for (status, proof) in [
        (LockStatus::RecoveredFromChain, true),
        (LockStatus::InstantSendLocked, true),
        (LockStatus::Broadcast, false),
    ] {
        let l = KindLock {
            info: LockInfo {
                status,
                ..second.info.clone()
            },
            proof,
            ..second.clone()
        };
        assert_eq!(
            Ev::default().state_of(&l),
            Some((LockOrigin::Restored, UnfinishedState::Unsupported)),
            "{status:?}"
        );
    }
    let kind = FakeKind {
        locks: Mutex::new(vec![first.clone(), second.clone()]),
        own: Some(HashSet::new()),
        script: Some(dashcore::ScriptBuf::new()),
        ..FakeKind::default()
    };
    let asked = AtomicBool::new(false);
    let lease = || async {
        asked.store(true, Ordering::SeqCst);
        Err::<Lease, _>(PlatformError::Unavailable)
    };
    assert_eq!(
        block_on(finish(&kind, second.info.outpoint, lease)).unwrap(),
        LockFinish::NotApplied {
            error: UNSUPPORTED.into()
        }
    );
    assert!(!asked.load(Ordering::SeqCst), "refused before the lease");
    assert_eq!(kind.applies.load(Ordering::SeqCst), 0);
    let lease = || async {
        asked.store(true, Ordering::SeqCst);
        Err::<Lease, _>(PlatformError::Unavailable)
    };
    let _ = block_on(finish(&kind, first.info.outpoint, lease));
    assert!(
        asked.load(Ordering::SeqCst),
        "output 0 goes on to its lease"
    );
}

/// Opus minor-3: a restore files another device's in-flight lock as
/// `InstantSendLocked` with no proof; it lists Confirming, and a finish
/// waits rather than redeeming the grant for the library's refusal.
#[test]
fn a_restored_instant_send_row_without_its_proof_is_confirming() {
    let unproved = KindLock {
        proof: false,
        ..alone(&lock(1, LockStatus::InstantSendLocked))
    };
    assert_eq!(
        Ev::default().state_of(&unproved),
        Some((LockOrigin::Restored, UnfinishedState::Confirming))
    );
    let proved = alone(&lock(1, LockStatus::InstantSendLocked));
    assert_eq!(
        Ev::default().state_of(&proved),
        Some((LockOrigin::Restored, UnfinishedState::Ready))
    );
    let kind = FakeKind {
        locks: Mutex::new(vec![unproved.clone()]),
        own: Some(HashSet::new()),
        script: Some(dashcore::ScriptBuf::new()),
        ..FakeKind::default()
    };
    let asked = AtomicBool::new(false);
    let lease = || async {
        asked.store(true, Ordering::SeqCst);
        Err::<Lease, _>(PlatformError::Unavailable)
    };
    assert_eq!(
        block_on(finish(&kind, unproved.info.outpoint, lease)).unwrap(),
        LockFinish::ProofWaiting
    );
    assert!(!asked.load(Ordering::SeqCst));
}

/// A lock worth less than the apply's minimum lists apart, too small to
/// finish (never Ready), and a finish refuses it with the kind's code before
/// the lease, so the grant is kept; a lock at the minimum is finishable. One
/// whose apply may have executed reads Checking, and is refused alike.
#[test]
fn a_lock_below_the_minimum_is_too_small_and_keeps_the_grant() {
    let small = alone(&lock(1, LockStatus::ChainLocked));
    let at_min = alone(&lock(2, LockStatus::ChainLocked));
    let min = Minimum {
        duffs: at_min.info.amount,
        code: "kind.too_small",
    };
    let ev = Ev {
        minimum: Some(min),
        ..Ev::default()
    };
    assert_eq!(
        ev.state_of(&small),
        Some((LockOrigin::Restored, UnfinishedState::TooSmall))
    );
    assert_eq!(
        ev.state_of(&at_min),
        Some((LockOrigin::Restored, UnfinishedState::Ready))
    );
    assert_eq!(
        Ev::default().state_of(&small),
        Some((LockOrigin::Restored, UnfinishedState::Ready)),
        "no minimum read, nothing is told too small"
    );
    let mut checked = Ev {
        minimum: Some(min),
        ..Ev::default()
    };
    checked.unknown.outpoints.insert(small.info.outpoint);
    assert_eq!(
        checked.state_of(&small),
        Some((LockOrigin::Restored, UnfinishedState::Checking))
    );
    for unknown in [false, true] {
        let kind = FakeKind {
            locks: Mutex::new(vec![small.clone()]),
            own: Some(HashSet::new()),
            minimum: Some(min),
            script: Some(dashcore::ScriptBuf::new()),
            ..FakeKind::default()
        };
        if unknown {
            block_on(kind.mark_unknown(&small));
        }
        let asked = AtomicBool::new(false);
        let lease = || async {
            asked.store(true, Ordering::SeqCst);
            Err::<Lease, _>(PlatformError::Unavailable)
        };
        assert_eq!(
            block_on(finish(&kind, small.info.outpoint, lease)).unwrap(),
            LockFinish::NotApplied {
                error: "kind.too_small".into()
            },
            "checking: {unknown}"
        );
        assert!(!asked.load(Ordering::SeqCst));
        assert_eq!(kind.applies.load(Ordering::SeqCst), 0);
    }
}

/// The refusals before a lease: a kind held back, and a lock whose
/// transaction is not tracked. Neither asks for the lease.
#[test]
fn a_finish_held_back_or_untracked_asks_for_no_lease() {
    let at = lock(1, LockStatus::ChainLocked);
    let asked = AtomicBool::new(false);
    let lease = || async {
        asked.store(true, Ordering::SeqCst);
        Err::<Lease, _>(PlatformError::Unavailable)
    };
    let held = FakeKind {
        locks: rows(std::slice::from_ref(&at)),
        own: Some(HashSet::new()),
        held: Some("shielded.not_synced".into()),
        script: Some(dashcore::ScriptBuf::new()),
        ..FakeKind::default()
    };
    assert_eq!(
        block_on(finish(&held, at.outpoint, lease)).unwrap(),
        LockFinish::NotApplied {
            error: "shielded.not_synced".into()
        }
    );
    let untracked = FakeKind {
        locks: rows(std::slice::from_ref(&at)),
        own: Some(HashSet::new()),
        ..FakeKind::default()
    };
    let lease = || async {
        asked.store(true, Ordering::SeqCst);
        Err::<Lease, _>(PlatformError::Unavailable)
    };
    assert_eq!(
        block_on(finish(&untracked, at.outpoint, lease)).unwrap_err(),
        PlatformError::Unavailable
    );
    assert!(!asked.load(Ordering::SeqCst), "no lease asked for");
    assert_eq!(held.applies.load(Ordering::SeqCst), 0);
    assert_eq!(untracked.applies.load(Ordering::SeqCst), 0);
}

/// Q2, the runner's half (type 5's own path through `ShieldLocks`,
/// `Witnessed` and the library's credit script is
/// `shielded_tests::unfinished`): the runner hands the kind's apply a
/// signer that signs with the lock's credit key (the script the lock's
/// payload names) and no other, under a real `Shield` lease of the vault: a
/// Core address key and another lock's credit key are refused by the
/// narrowing (`KeyMismatch`), an identity authentication key and an
/// operator key by the lease's vault scope, before the narrowing reads them.
#[test]
fn the_runner_signs_with_the_lock_key_only() {
    use std::str::FromStr;

    use key_wallet::Signer as _;

    let fx = super::requests::tests::Fixture::new(0);
    let grant = || {
        (fx.s.vault)
            .authorize(
                dw_vault::GrantPurpose::PlatformOp {
                    max_duffs: 1,
                    max_credits: 0,
                },
                Some(&fx.wallet.0),
                dw_vault::Credential::None,
            )
            .unwrap()
            .id
    };
    let (probe, used) = (grant(), grant());
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    let path = |p: &str| DerivationPath::from_str(p).unwrap();
    let (lock_key, other_lock, core) = (
        path("m/9'/1'/5'/5'/0"),
        path("m/9'/1'/5'/5'/1"),
        path("m/44'/1'/0'/0/0"),
    );
    let (identity, operator) = (path("m/9'/1'/5'/0'/0'/0'/0'"), path("m/9'/1'/3'/3'/0'"));
    let at = lock(1, LockStatus::ChainLocked);
    let (finished, signed) = fx.engine.block_on(async move {
        let script = {
            let lease = (s.begin_lease(wallet, FlowKind::Shield, &[probe]).await).unwrap();
            let funding = lease.uncounted_funding_signer().unwrap();
            let key = funding.public_key(&lock_key).await.unwrap();
            dashcore::ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash())
        };
        let kind = FakeKind {
            locks: rows(std::slice::from_ref(&at)),
            own: Some(HashSet::new()),
            script: Some(script),
            signs: vec![core, other_lock, identity, operator, lock_key],
            ..FakeKind::default()
        };
        let lease = || async move {
            Ok::<_, PlatformError>(s.begin_lease(wallet, FlowKind::Shield, &[used]).await?)
        };
        let finished = finish(&kind, at.outpoint, lease).await;
        (finished, kind.signed.lock().unwrap().clone())
    });
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert_eq!(
        signed,
        [false, false, false, false, true],
        "only the lock's credit key signs"
    );
}

/// A FakeKind over a real `Shield` lease whose funding signer holds the
/// lock key `m/9'/1'/5'/5'/0`.
struct FakeRig {
    fx: super::requests::tests::Fixture,
    kind: FakeKind,
}

impl FakeRig {
    fn new() -> Self {
        use std::str::FromStr;
        let fx = super::requests::tests::Fixture::new(0);
        let lock_key = DerivationPath::from_str("m/9'/1'/5'/5'/0").unwrap();
        let grant = Self::grant(&fx);
        let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
        let script = fx.engine.block_on(async {
            let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant]).await).unwrap();
            let funding = lease.uncounted_funding_signer().unwrap();
            let key = key_wallet::Signer::public_key(&funding, &lock_key).await;
            let key = dashcore::PublicKey::new(key.unwrap());
            dashcore::ScriptBuf::new_p2pkh(&key.pubkey_hash())
        });
        let kind = FakeKind {
            own: Some(HashSet::new()),
            script: Some(script),
            signs: vec![lock_key],
            minimum: Some(Minimum {
                duffs: 1_000,
                code: "fake.too_small",
            }),
            ..FakeKind::default()
        };
        Self { fx, kind }
    }

    fn grant(fx: &super::requests::tests::Fixture) -> String {
        let purpose = dw_vault::GrantPurpose::PlatformOp {
            max_duffs: 1,
            max_credits: 0,
        };
        let granted =
            (fx.s.vault).authorize(purpose, Some(&fx.wallet.0), dw_vault::Credential::None);
        granted.unwrap().id
    }
}

impl Rig for FakeRig {
    fn track(&self, rows: Vec<KindLock>) {
        *self.kind.locks.lock().unwrap() = rows;
    }

    fn next(&self, end: End) {
        self.kind.ends.lock().unwrap().push_back(end);
    }

    fn applies(&self) -> usize {
        self.kind.applies.load(Ordering::SeqCst) as usize
    }

    fn list(&self) -> Result<Vec<UnfinishedLock>, PlatformError> {
        self.fx.engine.block_on(detect(&self.kind, &HashSet::new()))
    }

    fn finish(&self, outpoint: OutPoint) -> (Result<LockFinish, PlatformError>, bool) {
        let (s, wallet, grant) = (
            Arc::clone(&self.fx.s),
            self.fx.wallet,
            Self::grant(&self.fx),
        );
        let asked = AtomicBool::new(false);
        let lease = || async {
            asked.store(true, Ordering::SeqCst);
            Ok::<_, PlatformError>(s.begin_lease(wallet, FlowKind::Shield, &[grant]).await?)
        };
        let finished = self.fx.engine.block_on(finish(&self.kind, outpoint, lease));
        (finished, asked.load(Ordering::SeqCst))
    }

    fn marks(&self) -> (Marks, Marks) {
        let kind = &self.kind;
        (
            kind.settled.lock().unwrap().clone(),
            kind.unknown.lock().unwrap().clone(),
        )
    }

    fn minimum(&self) -> Option<Minimum> {
        self.kind.minimum
    }
}

/// The runner's own conformance: a kind that returns a failure after its
/// signature as an error still reads Checking (Opus minor-2), and the rest
/// of the contract holds over the scripted kind.
#[test]
fn the_runner_conforms() {
    conformance::conformance(&FakeRig::new());
}

/// Opus minor-2 on its own: a kind that signs, then returns the failure as
/// an error, cannot make a possibly executed apply read Ready.
#[test]
fn an_error_after_the_signature_is_checking_whatever_the_kind_says() {
    let rig = FakeRig::new();
    let at = conformance::row(1, 0, LockStatus::RecoveredFromChain, true, 1);
    rig.track(vec![at.clone()]);
    rig.next(End::SignedThenFailed);
    assert_eq!(
        rig.finish(at.info.outpoint).0.unwrap(),
        LockFinish::Checking
    );
    assert_eq!(*rig.kind.signed.lock().unwrap(), [true]);
    assert!(rig.marks().1.outpoints.contains(&at.info.outpoint));
    let listed = rig.list().unwrap();
    assert_eq!(listed[0].state, UnfinishedState::Checking);
}
