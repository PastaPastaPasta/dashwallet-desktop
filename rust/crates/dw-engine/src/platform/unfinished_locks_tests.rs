//! Tests of unfinished_locks.rs: a lock's standing from the library's row
//! and the engine's evidence, the one reply that reads "already consumed",
//! and detection over a scripted kind.

use std::collections::HashSet;
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
use super::unfinished_locks::{
    ApplyReply, Evidence, LockFinish, LockKeySigner, LockKind, LockKindId, LockOrigin, Readiness,
    UnfinishedState, classify, detect, finish, reply_from,
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

/// The evidence, empty but for what `f` adds.
struct Ev {
    own: HashSet<Txid>,
    settled: HashSet<Txid>,
    unknown: HashSet<Txid>,
    shared: HashSet<Txid>,
    used: HashSet<OutPoint>,
    readiness: Readiness,
}

impl Default for Ev {
    fn default() -> Self {
        Self {
            own: HashSet::new(),
            settled: HashSet::new(),
            unknown: HashSet::new(),
            shared: HashSet::new(),
            used: HashSet::new(),
            readiness: Readiness::Ready,
        }
    }
}

impl Ev {
    fn state(&self, l: &LockInfo) -> Option<(LockOrigin, UnfinishedState)> {
        let ev = Evidence {
            own: &self.own,
            settled: &self.settled,
            unknown: &self.unknown,
            shared: &self.shared,
            reported_used: &self.used,
            readiness: &self.readiness,
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
        settled: HashSet::from([settled.outpoint.txid]),
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
            unknown: HashSet::from([l.outpoint.txid]),
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
        unknown: HashSet::from([l.outpoint.txid]),
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

/// platform-wallet's tracked rows, scripted.
#[derive(Default)]
struct FakeKind {
    locks: Vec<LockInfo>,
    own: Option<HashSet<Txid>>,
    settled: HashSet<Txid>,
    unknown: Mutex<HashSet<Txid>>,
    /// What holds an apply back (`Readiness::Held`).
    held: Option<String>,
    /// The tracked credit script of every lock.
    script: Option<dashcore::ScriptBuf>,
    /// The keys an apply asks its signer for, and whether each signed.
    signs: Vec<DerivationPath>,
    signed: Mutex<Vec<bool>>,
    applies: AtomicU32,
}

impl LockKind for FakeKind {
    fn id(&self) -> LockKindId {
        LockKindId::Shielded
    }

    fn flow(&self) -> FlowKind {
        FlowKind::Shield
    }

    async fn locks(&self) -> Vec<LockInfo> {
        self.locks.clone()
    }

    fn own_locks(&self) -> Option<HashSet<Txid>> {
        self.own.clone()
    }

    async fn settled_locks(&self) -> HashSet<Txid> {
        self.settled.clone()
    }

    async fn unknown_locks(&self) -> HashSet<Txid> {
        self.unknown.lock().unwrap().clone()
    }

    async fn mark_unknown(&self, txid: Txid) {
        self.unknown.lock().unwrap().insert(txid);
    }

    async fn readiness(&self) -> Readiness {
        self.held.clone().map_or(Readiness::Ready, Readiness::Held)
    }

    fn not_active(&self) -> Option<String> {
        None
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

    /// Signs as the library's transition does, with every key of `signs`.
    async fn apply(
        &self,
        _: OutPoint,
        signer: &LockKeySigner<'_>,
    ) -> Result<ApplyReply, PlatformError> {
        use key_wallet::Signer as _;
        self.applies.fetch_add(1, Ordering::SeqCst);
        for path in &self.signs {
            let signed = signer.sign_ecdsa(path, [7; 32]).await.is_ok();
            self.signed.lock().unwrap().push(signed);
        }
        Ok(ApplyReply::Done)
    }

    async fn settle(&self, _: Txid) {}
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
        locks: vec![lock(1, LockStatus::RecoveredFromChain)],
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
        locks: vec![own.clone(), restored.clone(), done],
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
        let narrowed = LockKeySigner::new(&funding, script);
        for refused in [&core, &other_lock] {
            let e = narrowed.sign_ecdsa(refused, [7; 32]).await.unwrap_err();
            assert!(matches!(e, dw_vault::SignerError::KeyMismatch(_)), "{e:?}");
        }
        assert!(!narrowed.signed(), "a refusal is no signature");
        let (_, signed_with) = narrowed.sign_ecdsa(&lock_key, [7; 32]).await.unwrap();
        assert_eq!(signed_with, key);
        assert!(narrowed.signed());
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

/// A lock transaction with two credit outputs the wallet's pools pay is
/// two tracked locks of one txid: the journal's txid records (settled,
/// unknown) say nothing of either, so each stands on its own row.
#[test]
fn a_txid_of_two_locks_is_told_apart_by_outpoint() {
    let first = lock(1, LockStatus::ChainLocked);
    let second = LockInfo {
        outpoint: OutPoint::new(first.outpoint.txid, 1),
        ..first.clone()
    };
    let txid = HashSet::from([first.outpoint.txid]);
    let ev = Ev {
        settled: txid.clone(),
        unknown: txid.clone(),
        shared: txid.clone(),
        ..Ev::default()
    };
    for l in [&first, &second] {
        assert_eq!(
            ev.state(l),
            Some((LockOrigin::Restored, UnfinishedState::Ready))
        );
    }
    let consumed = LockInfo {
        status: LockStatus::Consumed,
        ..first.clone()
    };
    assert_eq!(ev.state(&consumed), None, "the library's row finishes one");

    let kind = FakeKind {
        locks: vec![second.clone(), first.clone()],
        own: Some(HashSet::new()),
        settled: txid,
        ..FakeKind::default()
    };
    let listed = block_on(detect(&kind, &HashSet::new())).unwrap();
    let vouts: Vec<_> = listed.iter().map(|u| u.vout).collect();
    assert_eq!(vouts, [0, 1]);
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
        locks: vec![at.clone()],
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
        locks: vec![at.clone()],
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

/// Q2 end to end: the runner hands the kind's apply a signer that signs
/// with the lock's credit key (the script the lock's payload names) and
/// refuses the funding scope's other keys, a Core address key and another
/// lock's credit key, under a real `Shield` lease of the vault.
#[test]
fn the_finish_path_signs_with_the_lock_key_only() {
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
    let at = lock(1, LockStatus::ChainLocked);
    let (finished, signed) = fx.engine.block_on(async move {
        let script = {
            let lease = (s.begin_lease(wallet, FlowKind::Shield, &[probe]).await).unwrap();
            let funding = lease.uncounted_funding_signer().unwrap();
            let key = funding.public_key(&lock_key).await.unwrap();
            dashcore::ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash())
        };
        let kind = FakeKind {
            locks: vec![at.clone()],
            own: Some(HashSet::new()),
            script: Some(script),
            signs: vec![core, other_lock, lock_key],
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
        [false, false, true],
        "only the lock's credit key signs"
    );
}
