//! The engine's dispatch hook (`shielded_pool::EngineDispatch`) over the
//! real library's coordinator and the real dispatch journal: what it
//! accepts before a spend's first broadcast, what a pass may re-send, the
//! hook's place on every open of the shielded store, and the refusals
//! while it is not set. No network: each test stages what the library
//! holds when it asks the hook (the account's notes and the stored, unsent
//! spend) and calls a hook built as [`pool::coordinator`] builds the one it
//! sets.

use std::sync::Arc;
use std::time::Duration;

use dw_appdb::dispatch::{
    ArtifactKind, ArtifactOrigin, ArtifactStatus, NewShieldedArtifact, ShieldedArtifact,
};
use dw_vault::SHIELDED_ACCOUNT;
use platform_wallet::wallet::shielded::store::PendingRedrive;
use platform_wallet::wallet::shielded::{
    DispatchedNote, NetworkShieldedCoordinator, ShieldedActivityEntry, ShieldedActivityKind,
    ShieldedActivityStatus, ShieldedDirection, ShieldedDispatch, ShieldedDispatchKind,
    ShieldedDispatchPayload, ShieldedStore, SubwalletId,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::super::shielded_pool::{self as pool, EngineDispatch, ShieldedLib, on_pool};
use super::super::topup_tests::{self, PASS, options};
use super::{NOTE, bound_address, core_address, note};
use crate::{DashNetwork, EngineError, ImportOptions, NetworkSession};

/// A nullifier no note of the account has: a padding action's.
const PADDING: [u8; 32] = [0xee; 32];

/// A second wallet's recovery phrase (BIP-39's test vector).
const OTHER_PHRASE: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

fn wait_until(what: impl Fn() -> bool, why: &str) {
    for _ in 0..4_000 {
        if what() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{why}");
}

/// A real session, its wallet bound through the facade's open (which
/// configures the coordinator and sets the hook), with a hook over the
/// same session and coordinator.
struct Hooked {
    f: topup_tests::Fixture,
    coordinator: Arc<NetworkShieldedCoordinator>,
    hook: Arc<EngineDispatch>,
}

impl Hooked {
    fn new() -> Self {
        let f = topup_tests::fixture(true);
        assert!(bound_address(&f.engine, &f.session, f.wallet).0);
        let manager = f.session.manager().unwrap();
        let coordinator = (f.engine.block_on(manager.shielded_coordinator())).unwrap();
        let hook = Arc::new(EngineDispatch::new(&f.session, &coordinator));
        Self {
            f,
            coordinator,
            hook,
        }
    }

    fn subwallet(&self) -> SubwalletId {
        SubwalletId::new(self.f.wallet.0, SHIELDED_ACCOUNT)
    }

    /// Gives the account the notes at `positions`.
    fn hold_notes(&self, positions: &[u64]) {
        let (coordinator, id) = (Arc::clone(&self.coordinator), self.subwallet());
        let notes: Vec<_> = positions.iter().map(|p| note(*p, NOTE)).collect();
        self.f.engine.block_on(async move {
            let mut store = coordinator.store().write().await;
            for n in &notes {
                store.save_note(id, n).unwrap();
            }
        });
    }

    /// Stores spend `n` unsent, as the library does before it asks the
    /// hook, reserving the notes at `positions`.
    fn store_unsent(&self, n: u8, positions: &[u64]) {
        let (coordinator, id) = (Arc::clone(&self.coordinator), self.subwallet());
        let redrive = PendingRedrive {
            activity_id: [n; 32],
            anchor: [0xa0; 32],
            nullifiers: positions.iter().map(|p| note(*p, NOTE).nullifier).collect(),
            st_bytes: st_bytes(n),
            attempts: 0,
            identity_nonce_finalized: false,
            identity_user_abandoned: false,
            anchor_height: Some(ANCHOR_HEIGHT),
        };
        self.f.engine.block_on(async move {
            (coordinator.store().write().await)
                .arm_redrive(id, redrive)
                .unwrap();
        });
    }

    /// The payload the library hands the hook for spend `n` selecting the
    /// notes at `positions`: one note is padded to two actions.
    fn payload(&self, n: u8, positions: &[u64]) -> ShieldedDispatchPayload {
        let selected_notes: Vec<_> = (positions.iter())
            .map(|p| note(*p, NOTE))
            .map(|n| DispatchedNote {
                nullifier: n.nullifier,
                cmx: n.cmx,
                value: n.value,
                position: n.position,
            })
            .collect();
        let mut published: Vec<_> = selected_notes.iter().map(|n| n.nullifier).collect();
        if published.len() == 1 {
            published.push(PADDING);
        }
        ShieldedDispatchPayload {
            kind: ShieldedDispatchKind::Transfer,
            wallet_id: self.f.wallet.0,
            account_index: SHIELDED_ACCOUNT,
            activity_id: [n; 32],
            st_hash: st_hash(n),
            st_bytes: st_bytes(n),
            anchor: [0xa0; 32],
            selected_notes,
            published_nullifiers: published,
        }
    }

    /// Notes, the stored spend and the payload of spend `n` taking the
    /// notes at `positions`, all consistent.
    fn staged(&self, n: u8, positions: &[u64]) -> ShieldedDispatchPayload {
        self.hold_notes(positions);
        self.store_unsent(n, positions);
        self.payload(n, positions)
    }

    fn dispatch(&self, payload: ShieldedDispatchPayload) -> Result<(), String> {
        let hook = Arc::clone(&self.hook);
        (self.f.engine)
            .block_on(async move { hook.dispatch(payload).await })
            .map_err(|e| e.reason)
    }

    fn admit(&self, account: u32, n: u8, hash: [u8; 32]) -> Result<(), String> {
        let (hook, wallet) = (Arc::clone(&self.hook), self.f.wallet.0);
        (self.f.engine)
            .block_on(async move { hook.admit(wallet, account, [n; 32], hash).await })
            .map_err(|e| e.reason)
    }

    fn artifact(&self, n: u8) -> Option<ShieldedArtifact> {
        artifact(&self.f, n)
    }

    /// Rewrites the journal row of spend `n` in place, as a later writer
    /// of its state would leave it: `set` is the SQL after `SET`.
    fn rewrite(&self, n: u8, set: &str) {
        let path = (self.f.session.data_dir()).join(dw_appdb::dispatch::DISPATCH_DB_FILE);
        let changed = rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                &format!("UPDATE shielded_artifact SET {set} WHERE wallet = ?1 AND activity = ?2"),
                rusqlite::params![&self.f.wallet.0[..], &[n; 32][..]],
            )
            .unwrap();
        assert_eq!(changed, 1, "one artifact row");
    }

    /// The wallet's background lease, created while the vault is unlocked.
    fn lease(&self) {
        let (s, wallet) = (Arc::clone(&self.f.session), self.f.wallet);
        self.f.engine.block_on(async move {
            s.ensure_background().await;
            assert!(s.leases.background(&wallet).is_some());
        });
    }

    /// Runs `during` while a quit (`Engine::shutdown`) has begun and is held
    /// at its first step, before the leases drop: the Platform supervisor's
    /// task, which the close cancels and awaits first
    /// (`PlatformRuntime::deactivate`), waits until `during` returns. The
    /// quit then completes.
    fn during_a_held_quit<T>(&self, during: impl FnOnce() -> T) -> T {
        use super::super::runtime::{Supervisor, guard};
        let (reached_tx, reached) = std::sync::mpsc::channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let (signals, _signals) = tokio::sync::mpsc::unbounded_channel();
        let task = {
            let release = Arc::clone(&release);
            self.f.session.rt.spawn(async move {
                let _ = cancelled.changed().await;
                let _ = reached_tx.send(());
                release.notified().await;
            })
        };
        *guard(&self.f.session.platform.supervisor) = Some(Supervisor {
            cancel,
            signals,
            task: Some(task),
            running: true,
        });
        std::thread::scope(|scope| {
            let quit = scope.spawn(|| self.f.engine.block_on(self.f.engine.shutdown()));
            reached
                .recv_timeout(Duration::from_secs(30))
                .expect("the quit never began");
            struct Release(Arc<tokio::sync::Notify>);
            impl Drop for Release {
                fn drop(&mut self) {
                    self.0.notify_one();
                }
            }
            let out = {
                let _release = Release(release);
                during()
            };
            quit.join().unwrap().unwrap();
            out
        })
    }

    /// The attempts of the stored spend `n`; `None` once it is gone.
    fn attempts(&self, n: u8) -> Option<u32> {
        let (coordinator, wallet) = (Arc::clone(&self.coordinator), self.f.wallet.0);
        (self.f.engine)
            .block_on(async move { stored_spends(&coordinator, wallet).await })
            .unwrap()
            .into_iter()
            .find(|(_, r)| r.activity_id == [n; 32])
            .map(|(_, r)| r.attempts)
    }
}

/// The proved height at which a test spend's anchor was in Platform's
/// recorded set, as the library keeps it with each record.
const ANCHOR_HEIGHT: u64 = 1_000;

/// `wallet`'s stored spend records, as the library holds them.
async fn stored_spends(
    coordinator: &NetworkShieldedCoordinator,
    wallet: [u8; 32],
) -> Result<Vec<(SubwalletId, PendingRedrive)>, String> {
    (coordinator.store().read().await)
        .pending_redrives_for_wallet(wallet)
        .map_err(|e| e.to_string())
}

/// What the library does with stored spend `n` the hook refused before its
/// first broadcast (operations.rs `settle_failed_spend`): its record retired
/// and its notes released, its history row failed.
fn refuse_undelivered<S: ShieldedStore>(store: &mut S, id: SubwalletId, n: u8)
where
    S::Error: std::fmt::Debug,
{
    let record = (store.pending_redrives(id).unwrap().into_iter())
        .find(|r| r.activity_id == [n; 32])
        .expect("the stored spend");
    assert!(store.retire_undelivered_spend(id, &[n; 32], &[]).unwrap());
    for nullifier in &record.nullifiers {
        store.clear_pending(id, nullifier).unwrap();
    }
    let entry = ShieldedActivityEntry {
        id: [n; 32],
        kind: ShieldedActivityKind::Sent,
        direction: ShieldedDirection::Out,
        amount: 1_000,
        fee: None,
        counterparty: None,
        memo: None,
        block_height: None,
        status: ShieldedActivityStatus::Failed,
        created_at_ms: 0,
        min_note_position: None,
        note_cmxs: Vec::new(),
        spent_nullifiers: record.nullifiers,
    };
    store.save_activity(id, &entry).unwrap();
}

fn st_bytes(n: u8) -> Vec<u8> {
    format!("signed transition {n}").into_bytes()
}

fn st_hash(n: u8) -> [u8; 32] {
    Sha256::digest(st_bytes(n)).into()
}

fn artifact(f: &topup_tests::Fixture, n: u8) -> Option<ShieldedArtifact> {
    (f.engine)
        .block_on(f.session.leases.shielded_artifact(f.wallet, [n; 32]))
        .unwrap()
}

fn nullifiers(positions: &[u64]) -> Vec<[u8; 32]> {
    positions.iter().map(|p| note(*p, NOTE).nullifier).collect()
}

/// An artifact's selected notes at `positions`, none seen spent.
fn unspent(positions: &[u64]) -> Vec<([u8; 32], bool)> {
    (nullifiers(positions).into_iter())
        .map(|n| (n, false))
        .collect()
}

/// Whether the engine set its dispatch hook on `coordinator`: the library
/// does not say.
fn recording(session: &NetworkSession, coordinator: &Arc<NetworkShieldedCoordinator>) -> bool {
    pool::hooked(session, coordinator)
}

/// The hook checks each selected note against the account's notes: a spend
/// taking a nullifier that is no note of the account (here the only one it
/// names, then one beside a real note) is refused, and nothing is written.
#[test]
fn h1_checks_real_notes_against_get_all_notes() {
    let h = Hooked::new();
    h.hold_notes(&[1]);
    // The library reserved what the payload names; only the account's
    // notes tell it apart.
    h.store_unsent(1, &[2]);
    let refused = h.dispatch(h.payload(1, &[2])).unwrap_err();
    assert!(refused.contains("does not hold"), "{refused}");
    assert_eq!(h.artifact(1), None);

    h.store_unsent(2, &[1, 3]);
    let refused = h.dispatch(h.payload(2, &[1, 3])).unwrap_err();
    assert!(refused.contains("does not hold"), "{refused}");
    assert_eq!(h.artifact(2), None);

    // The same spends once the account holds the notes.
    h.hold_notes(&[2, 3]);
    assert_eq!(h.dispatch(h.payload(1, &[2])), Ok(()));
    assert_eq!(h.dispatch(h.payload(2, &[1, 3])), Ok(()));
}

/// The selected notes must be exactly those the library reserved for the
/// spend: another note of the account in place of one (the same count), a
/// note more, a note fewer, another activity's spend and no stored spend
/// are refused, and nothing is written.
#[test]
fn h1_refuses_when_selected_differs_from_redrive() {
    let h = Hooked::new();
    h.hold_notes(&[1, 2, 3]);
    h.store_unsent(1, &[1, 2]);
    for selected in [&[1u64, 3][..], &[1, 2, 3], &[1]] {
        let refused = h.dispatch(h.payload(1, selected)).unwrap_err();
        assert!(
            refused.contains("not the one the library stored"),
            "{refused}"
        );
    }
    // The library stored nothing for activity 2.
    let refused = h.dispatch(h.payload(2, &[1, 2])).unwrap_err();
    assert!(refused.contains("stored no spend"), "{refused}");
    // The stored transition is another one.
    let mut other = h.payload(1, &[1, 2]);
    other.st_bytes = st_bytes(9);
    other.st_hash = st_hash(9);
    assert!(h.dispatch(other).is_err());
    // The hash is not that of the bytes.
    let mut forged = h.payload(1, &[1, 2]);
    forged.st_hash = st_hash(9);
    assert!(h.dispatch(forged).is_err());
    // Another account than the engine's.
    let mut account = h.payload(1, &[1, 2]);
    account.account_index = SHIELDED_ACCOUNT + 1;
    assert!(h.dispatch(account).is_err());
    assert_eq!((h.artifact(1), h.artifact(2)), (None, None));

    assert_eq!(h.dispatch(h.payload(1, &[1, 2])), Ok(()));
}

/// A one-note spend publishes a padding action's nullifier beside its
/// note's. The artifact keeps the two apart: the notes it holds are the
/// real ones only. A padding nullifier named as a selected note is
/// refused, and one reported spent settles and clears nothing.
#[test]
fn settle_evidence_reads_real_note_nullifiers_only() {
    use platform_wallet::changeset::{
        PlatformWalletChangeSet, PlatformWalletPersistence, ShieldedChangeSet,
    };
    let h = Hooked::new();
    let payload = h.staged(1, &[1]);
    assert_eq!(payload.published_nullifiers, [nullifiers(&[1])[0], PADDING]);

    // Padding named as a note: the account holds no such note.
    let mut padded = payload.clone();
    padded.selected_notes.push(DispatchedNote {
        nullifier: PADDING,
        cmx: [0; 32],
        value: 0,
        position: 99,
    });
    assert!(h.dispatch(padded).is_err());
    assert_eq!(h.artifact(1), None);

    assert_eq!(h.dispatch(payload), Ok(()));
    let written = h.artifact(1).unwrap();
    assert_eq!(written.selected, unspent(&[1]));
    assert_eq!(written.published, [nullifiers(&[1])[0], PADDING]);

    // The padding nullifier seen spent, through the store's tap.
    let mut shielded = ShieldedChangeSet::default();
    shielded
        .nullifiers_spent
        .insert(h.subwallet(), vec![PADDING]);
    let changeset = PlatformWalletChangeSet {
        shielded: Some(shielded),
        ..Default::default()
    };
    (h.f.session.live().unwrap().store)
        .store(h.f.wallet.0, changeset)
        .unwrap();
    assert_eq!(h.artifact(1), Some(written));
    assert_eq!(h.attempts(1), Some(0), "the stored spend stands");
    h.lease();
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
}

/// The artifact is in the journal when the hook returns `Ok`, never after:
/// while its write is held the hook has not answered and the journal has
/// no row; released, the hook answers `Ok` with the row written, as the
/// library handed it over. A second spend of the same activity is refused.
#[test]
fn h1_writes_the_artifact_before_returning_ok() {
    let h = Hooked::new();
    let payload = h.staged(1, &[1, 2]);
    let pause = Arc::new(crate::coins::TestPause::default());
    *h.f.session.leases.hold_artifact_write.lock().unwrap() = Some(Arc::clone(&pause));
    let (hook, held, s, wallet) = (
        Arc::clone(&h.hook),
        Arc::clone(&pause),
        Arc::clone(&h.f.session),
        h.f.wallet,
    );
    let answered = h.f.engine.block_on(async move {
        let call = tokio::spawn(async move { hook.dispatch(payload).await });
        let reached = async {
            while !held.reached.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        (tokio::time::timeout(Duration::from_secs(30), reached).await)
            .expect("the hook never wrote the artifact");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !call.is_finished(),
            "the hook answered before the artifact was written"
        );
        assert_eq!(s.leases.shielded_artifact(wallet, [1; 32]).await, Ok(None));
        held.release.notify_one();
        call.await.unwrap()
    });
    assert_eq!(answered, Ok(()));
    let written = h.artifact(1).unwrap();
    assert_eq!(
        (
            written.origin,
            written.st_hash,
            written.kind,
            written.anchor
        ),
        (
            ArtifactOrigin::Dispatched,
            Some(st_hash(1)),
            Some(ArtifactKind::Transfer),
            Some([0xa0; 32])
        )
    );
    assert_eq!(
        (written.status, written.disposition, written.acked),
        (ArtifactStatus::Registered, None, false)
    );
    assert_eq!(written.selected, unspent(&[1, 2]));
    assert_eq!(written.published, nullifiers(&[1, 2]));
    assert_eq!(h.f.session.leases.journal_synchronous(), Some(2), "FULL");

    // The first record stands: the same activity is not accepted twice.
    let refused = h.dispatch(h.payload(1, &[1, 2])).unwrap_err();
    assert!(refused.contains("already has an artifact"), "{refused}");

    // No journal, no send.
    let unwritten = h.staged(2, &[3]);
    let journal = h.f.session.leases.journal.get();
    h.f.session.leases.journal.set(None);
    assert!(h.dispatch(unwritten).is_err());
    h.f.session.leases.journal.set(journal);
    assert_eq!(h.artifact(2), None);
}

/// A pass may re-send only what the hook accepted: the artifact's wallet,
/// activity and hash. Another hash, an activity with no artifact, another
/// account and a journal that cannot be read are refused, and the stored
/// spend is left as it was.
#[test]
fn admit_refuses_an_unknown_st_hash() {
    let h = Hooked::new();
    h.lease();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));

    assert!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(2)).is_err());
    let unknown = h.admit(SHIELDED_ACCOUNT, 2, st_hash(1)).unwrap_err();
    assert!(unknown.contains("no artifact"), "{unknown}");
    assert!(h.admit(SHIELDED_ACCOUNT + 1, 1, st_hash(1)).is_err());
    // A spend the library stored and the hook never accepted.
    h.hold_notes(&[2]);
    h.store_unsent(3, &[2]);
    assert!(h.admit(SHIELDED_ACCOUNT, 3, st_hash(3)).is_err());

    let journal = h.f.session.leases.journal.get();
    h.f.session.leases.journal.set(None);
    let unread = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
    assert!(unread.contains("could not be read"), "{unread}");
    h.f.session.leases.journal.set(journal);

    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
    assert_eq!((h.attempts(1), h.attempts(3)), (Some(0), Some(0)));
}

/// A row imported from the 0.1 journal is never a reason to send: with the
/// wallet, the activity and the hash all matching, `admit` refuses it.
#[test]
fn admit_refuses_an_imported_row() {
    let h = Hooked::new();
    h.lease();
    let journal = h.f.session.leases.journal.get().unwrap();
    for (n, origin) in [
        (1, ArtifactOrigin::ImportedRedrive),
        (2, ArtifactOrigin::ImportedMarker),
    ] {
        let imported = NewShieldedArtifact {
            activity: [n; 32],
            origin,
            st_hash: Some(st_hash(n)),
            kind: None,
            anchor: Some([0xa0; 32]),
            identity_id: None,
            selected: nullifiers(&[1]),
            published: Vec::new(),
        };
        assert_eq!(
            journal.record_shielded_artifact(&h.f.wallet.0, &imported),
            Ok(true)
        );
        // Classified, as the session's load leaves an imported spend: the
        // state a dispatched artifact is re-sent from.
        h.rewrite(n, "status = 'unresolved', status_reason = NULL");
        let row = h.artifact(n).unwrap();
        assert_eq!(
            (row.origin, row.status, row.st_hash, row.disposition),
            (
                origin,
                ArtifactStatus::Unresolved { unrecorded: false },
                Some(st_hash(n)),
                None
            )
        );
        let refused = h.admit(SHIELDED_ACCOUNT, n, st_hash(n)).unwrap_err();
        assert!(refused.contains("does not allow"), "{refused}");
    }
    // The same state written by the hook is re-sent.
    assert_eq!(h.dispatch(h.staged(3, &[3])), Ok(()));
    h.rewrite(3, "status = 'unresolved', status_reason = NULL");
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 3, st_hash(3)), Ok(()));
}

/// An artifact the library's verdict is written on is never re-sent,
/// whatever the verdict: executed, not executed or unresolved. Without one
/// the same artifact is.
#[test]
fn admit_refuses_an_artifact_with_a_disposition() {
    let h = Hooked::new();
    h.lease();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    for (kind, reason) in [
        ("'unresolved'", "'cancelled'"),
        ("'not_executed'", "'refused'"),
        ("'executed'", "NULL"),
    ] {
        h.rewrite(
            1,
            &format!("disposition_kind = {kind}, disposition_reason = {reason}"),
        );
        let row = h.artifact(1).unwrap();
        assert_eq!(row.status, ArtifactStatus::Registered);
        assert!(row.disposition.is_some());
        let refused = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
        assert!(refused.contains("does not allow"), "{kind}: {refused}");
    }
    h.rewrite(1, "disposition_kind = NULL, disposition_reason = NULL");
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
    assert_eq!(h.attempts(1), Some(0));
}

/// Only a registered artifact and one whose result is not known are
/// re-sent: an archived one (its wallet was erased), a settled one, one not
/// classified and one marked unrecorded are refused.
#[test]
fn admit_refuses_an_archived_or_settled_artifact() {
    let h = Hooked::new();
    h.lease();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    for (set, status) in [
        ("status = 'archived'", ArtifactStatus::Archived),
        ("status = 'executed'", ArtifactStatus::Executed),
        ("status = 'not_executed'", ArtifactStatus::NotExecuted),
        ("status = 'unclassified'", ArtifactStatus::Unclassified),
        (
            "status = 'unresolved', status_reason = 'unrecorded'",
            ArtifactStatus::Unresolved { unrecorded: true },
        ),
    ] {
        h.rewrite(1, "status = 'registered', status_reason = NULL");
        h.rewrite(1, set);
        let row = h.artifact(1).unwrap();
        assert_eq!(
            (row.origin, row.status, row.disposition),
            (ArtifactOrigin::Dispatched, status, None)
        );
        let refused = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
        assert!(refused.contains("does not allow"), "{status:?}: {refused}");
    }
    for set in [
        "status = 'unresolved', status_reason = NULL",
        "status = 'registered', status_reason = NULL",
    ] {
        h.rewrite(1, set);
        assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
    }
    assert_eq!(h.attempts(1), Some(0));
}

/// A note named twice, or no note at all, is refused: the reserved set
/// would otherwise match a selection that is not the one named.
#[test]
fn h1_refuses_a_note_selected_twice() {
    let h = Hooked::new();
    h.hold_notes(&[1]);
    h.store_unsent(1, &[1]);
    let refused = h.dispatch(h.payload(1, &[1, 1])).unwrap_err();
    assert!(refused.contains("one twice"), "{refused}");
    let refused = h.dispatch(h.payload(1, &[])).unwrap_err();
    assert!(refused.contains("names no notes"), "{refused}");
    assert_eq!(h.artifact(1), None);
    assert_eq!(h.dispatch(h.payload(1, &[1])), Ok(()));
}

/// `admit` needs the wallet's background lease: refused once a lock drops
/// it, allowed again after the unlock brings it back.
#[test]
fn admit_refuses_without_the_wallets_lease() {
    let h = Hooked::new();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    h.lease();
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));

    h.f.engine.block_on(h.f.session.lock_vault()).unwrap();
    assert!(h.f.session.leases.background(&h.f.wallet).is_none());
    let refused = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
    assert!(refused.contains("background lease"), "{refused}");
    assert_eq!(h.attempts(1), Some(0));

    (h.f.engine)
        .block_on(
            h.f.session
                .vault_op(|v| v.unlock(PASS, dw_vault::UnlockScope::Full)),
        )
        .unwrap();
    h.lease();
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
}

/// A marked spend can let the pass scan while the vault is locked, but its
/// re-send is refused by `admit` until the background lease exists again.
#[test]
fn a_pass_runs_without_the_lease_when_every_stored_spend_asks_the_hook() {
    let h = Hooked::new();
    assert_eq!(h.dispatch(h.staged(8, &[1])), Ok(()));
    h.f.engine.block_on(h.f.session.lock_vault()).unwrap();
    let manager = h.f.session.manager().unwrap();
    let s = Arc::clone(&h.f.session);
    h.f.engine.block_on(async move {
        assert!(pool::admit_pass(&s, &manager).await.is_ok());
    });
    let refused = h.admit(SHIELDED_ACCOUNT, 8, st_hash(8)).unwrap_err();
    assert!(refused.contains("background lease"), "{refused}");
    // No pass runs here: this pins only that `admit` itself sends nothing.
    assert_eq!(h.attempts(8), Some(0));

    h.f.engine
        .block_on(
            h.f.session
                .vault_op(|v| v.unlock(PASS, dw_vault::UnlockScope::Full)),
        )
        .unwrap();
    h.lease();
    let manager = h.f.session.manager().unwrap();
    let s = Arc::clone(&h.f.session);
    h.f.engine.block_on(async move {
        assert!(pool::admit_pass(&s, &manager).await.is_ok());
    });
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 8, st_hash(8)), Ok(()));
}

/// The fork held a pass without the wallet's lease while a record armed
/// before the hook was set (re-sent without asking it) stood beside a hooked
/// one. Upstream asks `admit` before every re-send, records stored before
/// the hook included, and `admit` requires the lease
/// (`a_pass_runs_without_the_lease_when_every_stored_spend_asks_the_hook`):
/// no record is re-sent without asking. The fork's body is kept below, not
/// compiled.
#[test]
#[ignore = "P13b: every stored spend asks admit, so no record predates the hook and no pass is held for one"]
fn a_pass_is_held_without_the_lease_while_a_spend_predates_the_hook() {
    panic!("P13b: no hookless record on upstream");
}

#[cfg(any())]
/// Any unmarked (pre-hook) record keeps the wallet-wide lease hold, including
/// when a marked hooked record is present beside it.
#[test]
fn a_pass_is_held_without_the_lease_while_a_spend_predates_the_hook() {
    let f = topup_tests::fixture(true);
    let manager = f.session.manager().unwrap();
    let wallet = f.wallet;
    let s = Arc::clone(&f.session);
    f.engine.block_on(async move {
        manager
            .configure_shielded(s.data_dir().join(pool::SHIELDED_DB_FILE))
            .await
            .unwrap();
        let c = manager.shielded_coordinator().await.unwrap();
        c.store()
            .write()
            .await
            .arm_redrive(
                SubwalletId::new(wallet.0, SHIELDED_ACCOUNT),
                PendingRedrive {
                    activity_id: [9; 32],
                    anchor: [0xa0; 32],
                    nullifiers: vec![[101; 32]],
                    st_bytes: b"pre-hook".to_vec(),
                    attempts: 0,
                    identity_nonce_finalized: false,
                    identity_user_abandoned: false,
                },
            )
            .unwrap();
        pool::coordinator(&s, &manager, None).await.unwrap();
        c.store()
            .write()
            .await
            .arm_unsent_redrive(
                SubwalletId::new(wallet.0, SHIELDED_ACCOUNT),
                PendingRedrive {
                    activity_id: [10; 32],
                    anchor: [0xa1; 32],
                    nullifiers: vec![[102; 32]],
                    st_bytes: b"hooked transition".to_vec(),
                    attempts: 0,
                    identity_nonce_finalized: false,
                    identity_user_abandoned: false,
                },
            )
            .unwrap();
        s.leases
            .record_shielded_artifact(
                wallet,
                NewShieldedArtifact {
                    activity: [10; 32],
                    origin: ArtifactOrigin::Dispatched,
                    st_hash: Some(Sha256::digest(b"hooked transition").into()),
                    kind: Some(ArtifactKind::Transfer),
                    anchor: Some([0xa1; 32]),
                    identity_id: None,
                    selected: vec![[102; 32]],
                    published: vec![[102; 32]],
                },
            )
            .await
            .unwrap();
        s.lock_vault().await.unwrap();
        assert_eq!(
            pool::admit_pass(&s, &manager).await.err(),
            Some(pool::PassHeld::NoLease(wallet))
        );
    });
}

/// Once the session's close has begun (a quit, held here before its leases
/// drop) the hook refuses both ways, with the wallet's lease still held: a
/// pass in flight re-sends nothing more (the stored spend keeps its
/// attempts) and no new spend is accepted or written. So it does once the
/// close has ended.
#[test]
fn close_during_redrive_pass_admit_refuses_without_attempt_bump() {
    let h = Hooked::new();
    h.lease();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    let unsent = h.staged(2, &[2]);
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));

    h.during_a_held_quit(|| {
        assert!(h.f.session.leases.background(&h.f.wallet).is_some());
        let refused = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
        assert!(refused.contains("closing"), "{refused}");
        let refused = h.dispatch(unsent).unwrap_err();
        assert!(refused.contains("closing"), "{refused}");
        assert_eq!((h.attempts(1), h.attempts(2)), (Some(0), Some(0)));
        assert_eq!(h.artifact(2), None);
    });
    assert!(!h.f.session.is_open());
    let refused = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
    assert!(refused.contains("closing"), "{refused}");
}

/// A quit begun between the library's store of a spend (unsent, its note
/// reserved) and the hook's answer: with the wallet's lease still held the
/// hook refuses it and writes nothing. The library then retires its record,
/// frees its note and fails its history row ([`refuse_undelivered`], staged
/// here: no spend built in a test reaches the network's hook), so nothing
/// was sent and nothing of it holds the wallet.
#[test]
fn quit_between_h2_and_h1_refuses_and_settles_not_sent() {
    let h = Hooked::new();
    h.lease();
    let payload = h.staged(1, &[1]);
    // The library's selection reserved the note for the spend.
    let (coordinator, id) = (Arc::clone(&h.coordinator), h.subwallet());
    h.f.engine.block_on(async move {
        let mut store = coordinator.store().write().await;
        let n = note(1, NOTE).nullifier;
        assert!(store.mark_pending(id, &n).unwrap());
        (store.set_pending_spend(id, &n, [0xa0; 32], [1; 32], Some(ANCHOR_HEIGHT))).unwrap();
    });
    h.during_a_held_quit(|| {
        assert!(h.f.session.leases.background(&h.f.wallet).is_some());
        let refused = h.dispatch(payload).unwrap_err();
        assert!(refused.contains("closing"), "{refused}");
        assert_eq!(h.artifact(1), None);

        let (coordinator, id, wallet) = (Arc::clone(&h.coordinator), h.subwallet(), h.f.wallet);
        let manager = h.f.session.manager().unwrap();
        h.f.engine.block_on(async move {
            let mut store = coordinator.store().write().await;
            assert!(store.get_unspent_notes(id).unwrap().is_empty(), "reserved");
            refuse_undelivered(&mut *store, id, 1);
            assert_eq!(store.get_unspent_notes(id).unwrap().len(), 1, "freed");
            assert!(store.stale_pending_spends(id).unwrap().is_empty());
            drop(store);
            assert!(
                stored_spends(&coordinator, wallet.0)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(!pool::holds_stored_spends(&manager, wallet).await);
        });
    });
}

/// While a revocation drains the passes (here another wallet's Close
/// Wallet, waiting for a pass in flight) the hook refuses for every wallet,
/// one that holds its lease too, and writes nothing; once the drain has
/// ended it answers again.
#[test]
fn a_drain_of_the_passes_refuses_the_hook() {
    let h = Hooked::new();
    let other = (h.f.engine)
        .block_on(h.f.session.import_wallet(
            Zeroizing::new(OTHER_PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap();
    h.lease();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    h.hold_notes(&[2]);
    h.store_unsent(2, &[2]);
    let passes = Arc::clone(&h.f.session.platform.shielded_passes);
    let in_flight = h.f.engine.block_on(passes.clone().read_owned());
    std::thread::scope(|scope| {
        let close = scope.spawn(|| h.f.engine.block_on(h.f.session.unload_wallet(other)));
        // The drain's write is queued behind the pass.
        wait_until(|| passes.try_read().is_err(), "the drain never began");
        assert!(h.f.session.leases.background(&h.f.wallet).is_some());
        let refused = h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)).unwrap_err();
        assert!(refused.contains("drain"), "{refused}");
        let refused = h.dispatch(h.payload(2, &[2])).unwrap_err();
        assert!(refused.contains("drain"), "{refused}");
        assert_eq!(h.artifact(2), None);
        drop(in_flight);
        close.join().unwrap().unwrap();
    });
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
    assert_eq!(h.dispatch(h.payload(2, &[2])), Ok(()));
}

/// An in-app close (the network switch's) is refused while a signing call
/// runs (`close_preflight`), so a dispatch in it is never cut: here the
/// close is asked while the hook waits to read the account's notes, and
/// the hook then accepts the spend and writes its artifact. The refused
/// close leaves the hook answering and the session open.
#[test]
fn close_during_dispatch_is_refused_and_dispatch_completes() {
    let h = Hooked::new();
    h.lease();
    let payload = h.staged(1, &[1]);
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (go, go_rx) = tokio::sync::oneshot::channel::<()>();
    let hook = Arc::clone(&h.hook);
    let call = pool::on_pool_signing(&h.f.session, h.f.wallet, move |_, _| async move {
        let _ = entered_tx.send(());
        let _ = go_rx.await;
        Ok(hook.dispatch(payload).await)
    });
    let (store_held_tx, store_held) = std::sync::mpsc::channel();
    let (release_store, release_rx) = tokio::sync::oneshot::channel::<()>();
    let coordinator = Arc::clone(&h.coordinator);
    std::thread::scope(|scope| {
        let call = scope.spawn(|| h.f.engine.block_on(call));
        entered.recv_timeout(Duration::from_secs(30)).unwrap();
        // The account's notes are not readable until the close was asked.
        h.f.session.rt.spawn(async move {
            let _store = coordinator.store().write().await;
            let _ = store_held_tx.send(());
            let _ = release_rx.await;
        });
        store_held.recv_timeout(Duration::from_secs(30)).unwrap();
        let _ = go.send(());
        std::thread::sleep(Duration::from_millis(50));
        let closed =
            h.f.engine
                .block_on(h.f.engine.close_network(DashNetwork::Regtest));
        assert!(
            matches!(closed, Err(EngineError::ShieldedSpendUnresolved)),
            "{closed:?}"
        );
        let _ = release_store.send(());
        assert_eq!(call.join().unwrap().unwrap(), Ok(()));
    });
    assert!(h.f.session.is_open());
    assert_eq!(
        h.artifact(1).map(|a| (a.origin, a.st_hash)),
        Some((ArtifactOrigin::Dispatched, Some(st_hash(1))))
    );
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));
}

/// A quit begun while a dispatch is past its first check (here waiting to
/// read the account's notes) still refuses the spend: the hook checks again
/// before it writes the artifact, so none is written and nothing is sent.
#[test]
fn a_quit_begun_mid_dispatch_refuses_it_before_the_artifact() {
    let h = Hooked::new();
    h.lease();
    let payload = h.staged(1, &[1]);
    let store = h.f.engine.block_on(h.coordinator.store().write());
    let hook = Arc::clone(&h.hook);
    let mut dispatch = Box::pin(async move { hook.dispatch(payload).await });
    let waiting = h.f.engine.block_on(std::future::poll_fn(|cx| {
        std::task::Poll::Ready(dispatch.as_mut().poll(cx).is_pending())
    }));
    assert!(waiting, "the dispatch waits for the account's notes");
    h.during_a_held_quit(|| {
        drop(store);
        let refused = h.f.engine.block_on(dispatch).unwrap_err().reason;
        assert!(refused.contains("closing"), "{refused}");
        assert_eq!(h.artifact(1), None);
    });
}

/// A quit fences every open network's hook before it closes the first: with
/// two networks open and the first close held at its first step, both hooks
/// already refuse.
#[test]
fn a_quit_fences_every_network_before_it_closes_the_first() {
    use super::super::runtime::{Supervisor, guard};
    let f = topup_tests::fixture(true);
    let testnet = (f.engine)
        .block_on(f.engine.open_network(DashNetwork::Testnet, options()))
        .unwrap();
    let sessions = [Arc::clone(&f.session), testnet];
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let mut receivers = Vec::new();
    let releases: Vec<_> = (sessions.iter())
        .map(|s| {
            let release = Arc::new(tokio::sync::Notify::new());
            let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
            let (signals, receiver) = tokio::sync::mpsc::unbounded_channel();
            receivers.push(receiver);
            let (reached_tx, wait) = (reached_tx.clone(), Arc::clone(&release));
            let task = s.rt.spawn(async move {
                let _ = cancelled.changed().await;
                let _ = reached_tx.send(());
                wait.notified().await;
            });
            *guard(&s.platform.supervisor) = Some(Supervisor {
                cancel,
                signals,
                task: Some(task),
                running: true,
            });
            release
        })
        .collect();
    let fenced = std::thread::scope(|scope| {
        let quit = scope.spawn(|| f.engine.block_on(f.engine.shutdown()));
        let began = reached.recv_timeout(Duration::from_secs(30));
        let fenced: Vec<_> = (sessions.iter())
            .map(|s| s.platform.hook_fence.refusal())
            .collect();
        for release in &releases {
            release.notify_one();
        }
        quit.join().unwrap().unwrap();
        began.expect("the quit never began");
        fenced
    });
    assert_eq!(fenced, [Some("the network session is closing"); 2]);
}

/// The hook is set by every open of the shielded store, before the
/// coordinator is handed out: the facade's first shielded call, the
/// Platform start, and each again after the session was closed and
/// reopened (the library keeps no hook across an open).
#[test]
fn the_hook_is_set_on_every_open_path() {
    // The facade's open.
    let mut f = topup_tests::fixture(true);
    let manager = f.session.manager().unwrap();
    assert!(f.engine.block_on(manager.shielded_coordinator()).is_none());
    let hooked = (f.engine)
        .block_on(on_pool(&f.session, f.wallet, |_, lib| async move {
            Ok(lib.hooked().await)
        }))
        .unwrap();
    assert!(hooked);
    // Bound, so the reopened session (its vault locked) binds from storage.
    assert!(bound_address(&f.engine, &f.session, f.wallet).0);
    let coordinator = (f.engine.block_on(manager.shielded_coordinator())).unwrap();
    assert!(recording(&f.session, &coordinator));
    drop((coordinator, manager));

    // A reopened session has a new store: the hook again, by the same call.
    (f.engine)
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.session = (f.engine)
        .block_on(f.engine.open_network(DashNetwork::Regtest, options()))
        .unwrap();
    let manager = f.session.manager().unwrap();
    assert!(f.engine.block_on(manager.shielded_coordinator()).is_none());
    assert!(bound_address(&f.engine, &f.session, f.wallet).0);
    let coordinator = (f.engine.block_on(manager.shielded_coordinator())).unwrap();
    assert!(recording(&f.session, &coordinator));

    // The Platform start.
    let started = topup_tests::fixture(true);
    let manager = started.session.manager().unwrap();
    assert!(
        (started.engine)
            .block_on(manager.shielded_coordinator())
            .is_none()
    );
    let (s, m, wallet) = (
        Arc::clone(&started.session),
        Arc::clone(&manager),
        started.wallet,
    );
    started.engine.block_on(async move {
        let (_cancel, cancelled) = tokio::sync::watch::channel(false);
        pool::start(&s, &m, &[wallet], &cancelled).await;
        let coordinator = (m.shielded_coordinator().await)
            .expect("the start opens the store on an active network");
        assert!(recording(&s, &coordinator));
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
}

/// A coordinator without the hook runs no pass, and is never handed out:
/// the next open sets the hook first.
#[test]
fn a_pass_is_held_while_the_hook_is_not_set_and_the_next_open_sets_it() {
    let f = topup_tests::fixture(true);
    let manager = f.session.manager().unwrap();
    let (s, m): (Arc<NetworkSession>, _) = (Arc::clone(&f.session), Arc::clone(&manager));
    f.engine.block_on(async move {
        // The store opened with no hook, as a failed set would leave it.
        m.configure_shielded(s.data_dir().join(pool::SHIELDED_DB_FILE))
            .await
            .unwrap();
        let bare = m.shielded_coordinator().await.unwrap();
        assert!(!recording(&s, &bare));
        assert_eq!(
            pool::admit_pass(&s, &m).await.err(),
            Some(pool::PassHeld::NoHook)
        );
        assert!(!pool::guarded_pass(&s, &m, true).await);

        let opened = pool::coordinator(&s, &m, None).await.unwrap();
        assert!(Arc::ptr_eq(&opened, &bare));
        assert!(recording(&s, &bare));
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        // Set once: a later open leaves it.
        pool::coordinator(&s, &m, None).await.unwrap();
        assert!(recording(&s, &bare));
    });
}

/// The library refuses a key set that is not the account's before it
/// selects, builds, stores or asks the hook anything, on both spends, and
/// the facade reads that as a refusal with nothing sent.
#[test]
#[ignore = "P13c: this platform-wallet has no key set spend entry point, so the engine refuses every spend before it releases a key"]
fn a_spend_with_a_mismatched_keyset_is_refused_and_sends_nothing() {
    panic!("needs the P13c key set spend entry points");
}

#[cfg(any())]
#[test]
fn a_spend_with_a_mismatched_keyset_is_refused_and_sends_nothing() {
    let h = Hooked::new();
    h.hold_notes(&[1, 2]);
    // Another wallet's key set (the payee's fixed test seed).
    let other = OrchardKeySet::from_seed(&[7; 64], dashcore::Network::Regtest, 0).unwrap();
    let (s, coordinator, wallet) = (
        Arc::clone(&h.f.session),
        Arc::clone(&h.coordinator),
        h.f.wallet,
    );
    let refusals = h.f.engine.block_on(async move {
        let pw = (s.manager().unwrap().get_wallet(&wallet.0).await).unwrap();
        let prover = CachedOrchardProver::new();
        let transfer = pw
            .shielded_transfer_to_with_keyset(
                &coordinator,
                &other,
                SHIELDED_ACCOUNT,
                &payee().to_raw_bytes(),
                NOTE / 2,
                [0; 36],
                &prover,
            )
            .await
            .unwrap_err();
        let withdraw = pw
            .shielded_withdraw_to_with_keyset(
                &coordinator,
                &other,
                SHIELDED_ACCOUNT,
                &core_address(1).to_string(),
                NOTE / 2,
                1,
                &prover,
            )
            .await
            .unwrap_err();
        // Nothing reserved, stored or resolved.
        let store = coordinator.store().read().await;
        let id = SubwalletId::new(wallet.0, SHIELDED_ACCOUNT);
        assert!(store.stale_pending_spends(id).unwrap().is_empty());
        assert_eq!(store.get_unspent_notes(id).unwrap().len(), 2);
        drop(store);
        assert!(
            (coordinator.pending_spend_redrives(wallet.0).await)
                .unwrap()
                .is_empty()
        );
        assert!(
            (coordinator.spend_dispositions(wallet.0).await)
                .unwrap()
                .is_empty()
        );
        [transfer, withdraw]
    });
    let journal = h.f.session.leases.journal.get().unwrap();
    assert_eq!(journal.shielded_artifacts(&h.f.wallet.0), Ok(Vec::new()));
    for refused in refusals {
        assert!(
            matches!(
                refused,
                PlatformWalletError::ShieldedKeysetMismatch {
                    account: SHIELDED_ACCOUNT
                }
            ),
            "{refused}"
        );
        assert_eq!(pool::spend_error(refused).code(), "shielded.spend_refused");
    }
}

/// A pending shielded payout's row for spend tests of `f`'s wallet.
fn payout_row(f: &topup_tests::Fixture, reference: &str) {
    let row = dw_appdb::DpContactPayoutRow {
        reference: reference.into(),
        identity: "6B8Dtk3Ut7cvQqSvK1Ys4fRqmDQEuoqCv5QrsrQAsh2j".into(),
        contact: "8iuLe3Ufbo8r9RNjN5dgK38fbQq5ZrQvEoyqEx6bBrph".into(),
        source: "shielded".into(),
        st_hash: None,
        address: core_address(4).to_string(),
        credits: NOTE,
        expected_duffs: 1_000,
        note: None,
        state: "pending".into(),
        since: 1,
    };
    let w = f.wallet.to_string();
    let written = (f.engine)
        .block_on(
            f.session
                .appdb_op(move |db| db.insert_dp_contact_payout_durable(&w, &row)),
        )
        .unwrap();
    assert_eq!(written, None);
}

/// The `st_hash` a payout's row names.
fn payout_hash(f: &topup_tests::Fixture, reference: &str) -> Option<String> {
    let w = f.wallet.to_string();
    (f.engine)
        .block_on(f.session.appdb_op(move |db| db.dp_contact_payouts(&w)))
        .unwrap()
        .into_iter()
        .find(|r| r.reference == reference)
        .and_then(|r| r.st_hash)
}

/// Whether the hook recorded the dispatch of payout `reference`.
fn dispatched(f: &topup_tests::Fixture, reference: &str) -> bool {
    let scope = dw_appdb::local_scope(&f.wallet.to_string());
    let key = dw_appdb::dp_payout_dispatched(reference);
    (f.engine)
        .block_on(f.session.appdb_op(move |db| db.setting(&scope, &key)))
        .unwrap()
        .is_some()
}

/// C-funding P6: with a contact payout armed, the hook names the spend's
/// transition in the payout's row, durably, before it writes the spend's
/// artifact, and records its dispatch once the artifact exists, before it
/// lets the library send; a spend that is not a withdrawal, or whose row
/// it cannot name, is refused with no artifact written, so nothing is
/// sent. Mutants: write the artifact first (the held write finds it);
/// skip the dispatch record.
#[test]
fn a_payout_spend_is_named_in_its_row_before_its_artifact() {
    let h = Hooked::new();
    let (session, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    payout_row(&h.f, "sh-a");
    let arm = session.arm_payout(wallet, "sh-a");

    let transfer = h.staged(1, &[1]);
    let refused = h.dispatch(transfer).unwrap_err();
    assert!(refused.contains("withdrawal"), "{refused}");
    assert!(h.artifact(1).is_none());
    assert_eq!(payout_hash(&h.f, "sh-a"), None);

    let mut withdrawal = h.staged(2, &[2]);
    withdrawal.kind = ShieldedDispatchKind::Withdraw;
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.platform.payments.hold_st_hash.lock().unwrap() = Some(Arc::clone(&pause));
    let hook = Arc::clone(&h.hook);
    let accepted = session
        .rt
        .spawn(async move { hook.dispatch(withdrawal).await });
    assert!(pause.wait_reached(Duration::from_secs(30)));
    assert!(
        h.artifact(2).is_none(),
        "no artifact before the row names it"
    );
    assert!(!dispatched(&h.f, "sh-a"));
    pause.release.notify_one();
    h.f.engine.block_on(accepted).unwrap().unwrap();
    assert!(
        dispatched(&h.f, "sh-a"),
        "recorded before the hook returned"
    );
    assert_eq!(payout_hash(&h.f, "sh-a"), Some(hex::encode(st_hash(2))));
    assert_eq!(h.artifact(2).unwrap().st_hash, Some(st_hash(2)));

    let mut again = h.staged(3, &[3]);
    again.kind = ShieldedDispatchKind::Withdraw;
    assert!(h.dispatch(again).unwrap_err().contains("accepted already"));
    assert!(h.artifact(3).is_none());
    drop(arm);

    let settle = |reference: &'static str, state: &'static str| {
        let w = wallet.to_string();
        (h.f.engine)
            .block_on(session.appdb_op(move |db| db.settle_dp_contact_payout(&w, reference, state)))
            .unwrap();
    };
    settle("sh-a", "sent");
    payout_row(&h.f, "sh-b");
    settle("sh-b", "not_sent");
    let _arm = session.arm_payout(wallet, "sh-b");
    let mut settled = h.staged(4, &[4]);
    settled.kind = ShieldedDispatchKind::Withdraw;
    assert!(h.dispatch(settled).unwrap_err().contains("not pending"));
    assert!(
        h.artifact(4).is_none(),
        "a refused accept writes no artifact"
    );
    assert_eq!(payout_hash(&h.f, "sh-b"), None);
}

/// P6: when the hook cannot record a payout's dispatch after writing its
/// artifact, it settles the artifact `not_executed` and refuses the spend,
/// so the library sends nothing and the journal proves it unsent.
#[test]
fn a_payout_spend_whose_dispatch_is_not_recorded_is_refused_unsent() {
    let h = Hooked::new();
    let (session, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    payout_row(&h.f, "sh-c");
    let _arm = session.arm_payout(wallet, "sh-c");
    (session.platform.payments.fail_dispatch_record)
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut withdrawal = h.staged(5, &[5]);
    withdrawal.kind = ShieldedDispatchKind::Withdraw;
    let refused = h.dispatch(withdrawal).unwrap_err();
    assert!(refused.contains("dispatch was not recorded"), "{refused}");
    assert_eq!(h.artifact(5).unwrap().status, ArtifactStatus::NotExecuted);
    assert!(!dispatched(&h.f, "sh-c"));
    assert_eq!(payout_hash(&h.f, "sh-c"), Some(hex::encode(st_hash(5))));
}

/// P6 at a re-send: a payout spend the hook accepted, whose recorded
/// dispatch is gone (a restart after a refusal whose artifact could not be
/// settled), is never re-sent; with its dispatch recorded it is.
#[test]
fn a_payout_spend_is_re_sent_only_with_its_dispatch_recorded() {
    let h = Hooked::new();
    h.lease();
    let (session, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    payout_row(&h.f, "sh-d");
    let arm = session.arm_payout(wallet, "sh-d");
    let mut withdrawal = h.staged(6, &[6]);
    withdrawal.kind = ShieldedDispatchKind::Withdraw;
    assert_eq!(h.dispatch(withdrawal), Ok(()));
    drop(arm);
    assert!(dispatched(&h.f, "sh-d"));
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 6, st_hash(6)), Ok(()));

    let set = |value: Option<&'static str>| {
        let scope = dw_appdb::local_scope(&wallet.to_string());
        let key = dw_appdb::dp_payout_dispatched("sh-d");
        (h.f.engine)
            .block_on(session.appdb_op(move |db| db.set_setting_durable(&scope, &key, value)))
            .unwrap();
    };
    set(None);
    let refused = h.admit(SHIELDED_ACCOUNT, 6, st_hash(6)).unwrap_err();
    assert!(refused.contains("dispatch was not recorded"), "{refused}");
    set(Some("1"));
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 6, st_hash(6)), Ok(()));
}

/// While key set spends are off, through the real library a send, a
/// withdrawal and an unshield are `platform.unavailable` before the grant
/// is read, and nothing is stored, reserved or written to the journal.
#[test]
fn every_spend_through_the_library_is_unavailable_while_key_set_spends_are_off() {
    let h = Hooked::new();
    pool::force_keyset_spends(&h.f.session, false);
    h.lease();
    h.hold_notes(&[1, 2]);
    let (s, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    for route in ["send", "withdraw", "unshield"] {
        let refused = (h.f.engine)
            .block_on(on_pool(&s, wallet, move |s, lib| async move {
                let code = match route {
                    "send" => {
                        let to = (super::payee_text(), NOTE);
                        (pool::send(&s, wallet, &lib, (&to.0, to.1), None, "unknown", false).await)
                            .map(|_| ())
                    }
                    "withdraw" => (pool::unshield(&s, wallet, &lib, NOTE, None, "unknown", false)
                        .await)
                        .map(|_| ()),
                    _ => {
                        (pool::unshield_to_platform(&s, wallet, &lib, NOTE, None, "unknown", false)
                            .await)
                            .map(|_| ())
                    }
                };
                Ok(code.unwrap_err().code())
            }))
            .unwrap();
        assert_eq!(refused, "platform.unavailable", "{route}");
    }
    let (coordinator, id) = (Arc::clone(&h.coordinator), h.subwallet());
    h.f.engine.block_on(async move {
        let store = coordinator.store().read().await;
        assert!(store.pending_redrives(id).unwrap().is_empty());
        assert!(store.stale_pending_spends(id).unwrap().is_empty());
        assert_eq!(store.get_unspent_notes(id).unwrap().len(), 2);
    });
    let artifacts = (h.f.engine)
        .block_on(s.leases.shielded_artifacts(wallet))
        .unwrap();
    assert!(artifacts.is_empty());
}

/// With key set spends on, a send through the real library spends with the
/// key set the vault derives from the wallet's own seed for that call: the
/// library takes it as the account's spend authority (no
/// `ShieldedKeysetMismatch`) and goes on past its notes' selection to the
/// network, where it fails offline before anything is stored or sent. The
/// notes it reserved are released and nothing is recorded or asked of the
/// hook.
#[test]
fn a_send_through_the_library_spends_with_the_wallets_own_key_set() {
    use platform_wallet::PlatformWalletError;
    let h = Hooked::new();
    pool::force_keyset_spends(&h.f.session, true);
    h.lease();
    h.hold_notes(&[1, 2]);
    let (coordinator, id) = (Arc::clone(&h.coordinator), h.subwallet());
    h.f.engine.block_on(async move {
        (coordinator.store().write().await)
            .set_last_synced_note_index(id, 3)
            .unwrap();
    });
    let grant = (h.f.session.vault())
        .authorize(
            dw_vault::GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: 2 * NOTE,
            },
            Some(&h.f.wallet.0),
            dw_vault::Credential::Passphrase(PASS),
        )
        .unwrap()
        .id;
    let (s, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    let failed = (h.f.engine)
        .block_on(on_pool(&s, wallet, move |s, lib| async move {
            let to = super::payee_text();
            let quoting = (NOTE / 2, pool::Quoting::Send(&to));
            let flow = super::super::flows::FlowKind::ShieldedSpend;
            let armed = pool::arm(&s, wallet, &lib, quoting, &grant, flow).await?;
            let permit = super::super::requests::write_permit(
                &armed.lease,
                "transfer",
                armed.quote.total_credits,
            )?;
            let sent = super::super::requests::leased(
                &armed.lease,
                lib.transfer(&permit, &super::payee(), NOTE / 2, [0; 36]),
            )
            .await;
            permit.finish(sent.is_ok());
            Ok(sent)
        }))
        .unwrap();
    let Err(pool::SpendError::Library(e)) = failed else {
        panic!("{failed:?}");
    };
    // Past the key set check and the notes' selection, the first network
    // call (the recorded anchors) finds no node.
    assert!(matches!(*e, PlatformWalletError::Sdk(_)), "{e:?}");
    let (coordinator, id) = (Arc::clone(&h.coordinator), h.subwallet());
    h.f.engine.block_on(async move {
        let store = coordinator.store().read().await;
        assert!(store.pending_redrives(id).unwrap().is_empty());
        assert!(store.stale_pending_spends(id).unwrap().is_empty());
        assert_eq!(store.get_unspent_notes(id).unwrap().len(), 2);
    });
    let artifacts = (h.f.engine)
        .block_on(s.leases.shielded_artifacts(wallet))
        .unwrap();
    assert!(artifacts.is_empty());
}

// ---- C-routes C1b-2: the unshield's FromShielded record ----

/// What unshield `n` taking the notes at `positions` signs: `to` credited
/// `amount` (the transition carries the fee on top).
fn unshield_bytes(
    n: u8,
    positions: &[u64],
    to: dpp::address_funds::PlatformAddress,
    amount: u64,
) -> Vec<u8> {
    use dpp::serialization::PlatformSerializable;
    use dpp::shielded::SerializedAction;
    use dpp::state_transition::StateTransition;
    use dpp::state_transition::unshield_transition::UnshieldTransition;
    use dpp::state_transition::unshield_transition::v0::UnshieldTransitionV0;
    let mut published = nullifiers(positions);
    if published.len() == 1 {
        published.push(PADDING);
    }
    let actions = (published.into_iter())
        .map(|nullifier| SerializedAction {
            nullifier,
            rk: [n; 32],
            cmx: [0x30 + n; 32],
            encrypted_note: vec![0x40 + n; 216],
            cv_net: [0x50 + n; 32],
            spend_auth_sig: [0x60 + n; 64],
        })
        .collect();
    let st = StateTransition::Unshield(UnshieldTransition::V0(UnshieldTransitionV0 {
        output_address: to,
        actions,
        unshielding_amount: amount,
        anchor: [0xa0; 32],
        proof: vec![0x70; 512],
        binding_signature: [0x71; 64],
    }));
    st.serialize_to_bytes().unwrap()
}

/// The fee these tests' unshields quote.
const UNSHIELD_FEE: u64 = 20_000_000;

/// A Platform address of the wallet's (in these tests, any one).
fn own_platform() -> dpp::address_funds::PlatformAddress {
    dpp::address_funds::PlatformAddress::P2pkh([0x51; 20])
}

impl Hooked {
    /// Notes, the stored unsent unshield and its payload: `to` credited
    /// `amount` with the fee, `bytes` its signed transition.
    fn staged_unshield(&self, n: u8, positions: &[u64], bytes: Vec<u8>) -> ShieldedDispatchPayload {
        self.hold_notes(positions);
        let (coordinator, id) = (Arc::clone(&self.coordinator), self.subwallet());
        let redrive = PendingRedrive {
            activity_id: [n; 32],
            anchor: [0xa0; 32],
            nullifiers: nullifiers(positions),
            st_bytes: bytes.clone(),
            attempts: 0,
            identity_nonce_finalized: false,
            identity_user_abandoned: false,
            anchor_height: Some(ANCHOR_HEIGHT),
        };
        self.f.engine.block_on(async move {
            (coordinator.store().write().await)
                .arm_redrive(id, redrive)
                .unwrap();
        });
        ShieldedDispatchPayload {
            kind: ShieldedDispatchKind::Unshield,
            st_hash: Sha256::digest(&bytes).into(),
            st_bytes: bytes,
            ..self.payload(n, positions)
        }
    }

    /// The wallet's unshield intent: `to`, the quoted credits, and the
    /// quoted total with [`UNSHIELD_FEE`].
    fn intend(&self, to: dpp::address_funds::PlatformAddress, credits: u64) {
        self.intend_total(to, credits, credits + UNSHIELD_FEE);
    }

    /// The wallet's unshield intent: `to`, `credits`, and the quoted
    /// `total`.
    fn intend_total(&self, to: dpp::address_funds::PlatformAddress, credits: u64, total: u64) {
        let intent = pool::UnshieldIntent {
            to,
            credits,
            total,
            record: None,
        };
        (self.f.session.platform.unshield_intents.lock().unwrap()).insert(self.f.wallet, intent);
    }

    fn intended_record(&self) -> Option<String> {
        (self.f.session.platform.unshield_intents.lock().unwrap())
            .get(&self.f.wallet)
            .and_then(|i| i.record.clone())
    }

    fn records(&self) -> Vec<super::super::platform_moves::FromShieldedRecord> {
        let (s, wallet) = (Arc::clone(&self.f.session), self.f.wallet);
        (self.f.engine).block_on(async move {
            super::super::platform_moves::from_shielded_records(&s, wallet).await
        })
    }
}

/// The hook's accept of an unshield writes its artifact, then its
/// `FromShielded` record armed (what the signed transition credits, as
/// the engine quoted it), before it answers; the record's row is noted
/// for the call's outcome.
#[test]
fn unshield_hook_arms_its_record_after_the_artifact() {
    use super::super::platform_moves::FromShieldedSource;
    let h = Hooked::new();
    let fee = UNSHIELD_FEE;
    let payload = h.staged_unshield(
        1,
        &[1],
        unshield_bytes(1, &[1], own_platform(), NOTE / 2 + fee),
    );
    let hash = payload.st_hash;
    h.intend(own_platform(), NOTE / 2);
    let again = payload.clone();
    assert_eq!(h.dispatch(payload), Ok(()));
    let artifact = h.artifact(1).unwrap();
    assert_eq!(
        (artifact.kind, artifact.st_hash),
        (Some(ArtifactKind::Unshield), Some(hash))
    );
    let records = h.records();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(
        (
            r.activity.as_str(),
            r.st_hash.as_str(),
            &r.source,
            r.credits,
            r.armed
        ),
        (
            hex::encode([1u8; 32]).as_str(),
            hex::encode(hash).as_str(),
            &FromShieldedSource::Unshield,
            NOTE / 2,
            true
        )
    );
    assert_eq!(
        r.to,
        own_platform().to_bech32m_string(dashcore::Network::Regtest)
    );
    assert_eq!(h.intended_record(), Some(r.id()));

    // The same unshield handed to the hook again: refused at its artifact,
    // its record as it was, still one.
    let refused = h.dispatch(again).unwrap_err();
    assert!(refused.contains("already has an artifact"), "{refused}");
    assert_eq!(h.records(), records);
}

/// No intent (an unshield the engine did not start), another address, or
/// a signed amount other than the quoted total: refused before the
/// artifact, nothing written, nothing sent.
#[test]
fn unshield_hook_refuses_without_or_against_its_intent() {
    let h = Hooked::new();
    let fee = UNSHIELD_FEE;
    use dpp::address_funds::PlatformAddress;
    let other = PlatformAddress::P2pkh([0x77; 20]);
    type Case = (Option<(PlatformAddress, u64)>, PlatformAddress, u64);
    let cases: [Case; 6] = [
        (None, own_platform(), NOTE / 2 + fee),
        (Some((own_platform(), NOTE / 2)), other, NOTE / 2 + fee),
        (Some((own_platform(), NOTE / 2)), own_platform(), NOTE / 2),
        (Some((own_platform(), NOTE)), own_platform(), NOTE / 2 + fee),
        // A signed total one credit off the quoted total, either way,
        // though still above the credits.
        (
            Some((own_platform(), NOTE / 2)),
            own_platform(),
            NOTE / 2 + fee + 1,
        ),
        (
            Some((own_platform(), NOTE / 2)),
            own_platform(),
            NOTE / 2 + fee - 1,
        ),
    ];
    for (n, (intent, to, amount)) in (1u8..).zip(cases) {
        (h.f.session.platform.unshield_intents.lock().unwrap()).clear();
        if let Some((to, credits)) = intent {
            h.intend(to, credits);
        }
        let payload = h.staged_unshield(
            n,
            &[u64::from(n)],
            unshield_bytes(n, &[u64::from(n)], to, amount),
        );
        assert!(h.dispatch(payload).is_err(), "case {n}");
        assert_eq!(h.artifact(n), None, "case {n}");
    }
    assert!(h.records().is_empty());
    // Bytes that are no unshield: refused too.
    h.intend(own_platform(), NOTE / 2);
    let payload = h.staged_unshield(9, &[9], st_bytes(9));
    assert!(h.dispatch(payload).is_err());
    assert_eq!(h.artifact(9), None);
}

/// A record that cannot be written refuses the send before its artifact:
/// nothing leaves unattributed (the library settles the spend not sent).
#[test]
fn unshield_hook_refuses_when_its_record_is_not_written() {
    let h = Hooked::new();
    let fee = UNSHIELD_FEE;
    let payload = h.staged_unshield(
        1,
        &[1],
        unshield_bytes(1, &[1], own_platform(), NOTE / 2 + fee),
    );
    h.intend(own_platform(), NOTE / 2);
    super::super::platform_moves::tests::fail_next_outcome_write_of(&h.f.session);
    let refused = h.dispatch(payload).unwrap_err();
    assert!(refused.contains("FromShielded record"), "{refused}");
    assert_eq!(h.artifact(1), None);
    assert!(h.records().is_empty());
    assert_eq!(h.intended_record(), None);
}

/// **A refusal between the artifact and the arming:** the record is
/// written before the artifact, so it is there unarmed (no row); the send
/// is refused and the artifact settled `not_executed` (the library retires
/// its record), so no re-send is admitted. **A stop between the artifact
/// and the arming** leaves both open: the pass's re-send is admitted only
/// once the hook armed the record. An unshield artifact with no record is
/// refused, as `dispatch` refuses one whose record is gone.
#[test]
fn an_unshield_refused_before_its_arming_is_settled_and_a_stopped_one_is_armed_at_re_send() {
    let h = Hooked::new();
    let fee = UNSHIELD_FEE;
    let payload = h.staged_unshield(
        1,
        &[1],
        unshield_bytes(1, &[1], own_platform(), NOTE / 2 + fee),
    );
    let hash = payload.st_hash;
    h.intend(own_platform(), NOTE / 2);
    super::super::platform_moves::tests::fail_next_arm_of(&h.f.session);
    let refused = h.dispatch(payload).unwrap_err();
    assert!(refused.contains("not armed"), "{refused}");
    assert_eq!(h.artifact(1).unwrap().status, ArtifactStatus::NotExecuted);
    let records = h.records();
    assert_eq!((records.len(), records[0].armed), (1, false));
    assert_eq!(h.intended_record(), None);
    h.lease();
    let refused = h.admit(SHIELDED_ACCOUNT, 1, hash).unwrap_err();
    assert!(refused.contains("does not allow a send"), "{refused}");
    assert!(!h.records()[0].armed);

    let (s, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    let stopped_hash = [0x33; 32];
    let record = super::super::platform_moves::FromShieldedRecord {
        activity: hex::encode([3; 32]),
        st_hash: hex::encode(stopped_hash),
        ..records[0].clone()
    };
    let stopped = dw_appdb::dispatch::NewShieldedArtifact {
        activity: [3; 32],
        origin: dw_appdb::dispatch::ArtifactOrigin::Dispatched,
        st_hash: Some(stopped_hash),
        kind: Some(ArtifactKind::Unshield),
        anchor: Some([0xa0; 32]),
        identity_id: None,
        selected: vec![[0x93; 32]],
        published: vec![[0x93; 32], [0xee; 32]],
    };
    let wrote = (h.f.engine).block_on(async move {
        super::super::platform_moves::stage_from_shielded(&s, wallet, record)
            .await
            .unwrap();
        s.leases.record_shielded_artifact(wallet, stopped).await
    });
    assert_eq!(wrote, Ok(true));
    let armed = |n: u8| {
        (h.records().into_iter())
            .find(|r| r.activity == hex::encode([n; 32]))
            .unwrap()
            .armed
    };
    assert!(!armed(3));
    super::super::platform_moves::tests::fail_next_arm_of(&h.f.session);
    let refused = h.admit(SHIELDED_ACCOUNT, 3, stopped_hash).unwrap_err();
    assert!(refused.contains("not armed"), "{refused}");
    assert!(!armed(3));
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 3, stopped_hash), Ok(()));
    assert!(armed(3), "armed before the re-send");

    let before = h.records().len();
    let (s, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    let legacy = dw_appdb::dispatch::NewShieldedArtifact {
        activity: [2; 32],
        origin: dw_appdb::dispatch::ArtifactOrigin::Dispatched,
        st_hash: Some([0x22; 32]),
        kind: Some(ArtifactKind::Unshield),
        anchor: Some([0xa0; 32]),
        identity_id: None,
        selected: vec![[0x92; 32]],
        published: vec![[0x92; 32], [0xee; 32]],
    };
    let wrote = (h.f.engine)
        .block_on(async move { s.leases.record_shielded_artifact(wallet, legacy).await });
    assert_eq!(wrote, Ok(true));
    let refused = h.admit(SHIELDED_ACCOUNT, 2, [0x22; 32]).unwrap_err();
    assert!(refused.contains("it is gone"), "{refused}");
    assert_eq!(h.records().len(), before);
}

/// A re-send whose `FromShielded` record is settled (failed or done) while
/// its artifact is still open is refused: nothing arms a settled record.
#[test]
fn an_unshield_whose_record_is_settled_is_not_admitted() {
    use super::super::platform_addresses::PlatformActivityState;
    let h = Hooked::new();
    let fee = UNSHIELD_FEE;
    for (n, state) in [
        (1u8, PlatformActivityState::Failed),
        (2, PlatformActivityState::Done),
    ] {
        let payload = h.staged_unshield(
            n,
            &[u64::from(n)],
            unshield_bytes(n, &[u64::from(n)], own_platform(), NOTE / 2 + fee),
        );
        let hash = payload.st_hash;
        h.intend(own_platform(), NOTE / 2);
        assert_eq!(h.dispatch(payload), Ok(()));
        h.lease();
        assert_eq!(h.admit(SHIELDED_ACCOUNT, n, hash), Ok(()));
        let (s, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
        (h.f.engine).block_on(async move {
            super::super::platform_moves::tests::set_record_state(&s, wallet, [n; 32], state).await
        });
        let refused = h.admit(SHIELDED_ACCOUNT, n, hash).unwrap_err();
        assert!(refused.contains("it is gone"), "{state:?}: {refused}");
        assert_eq!(h.artifact(n).unwrap().status, ArtifactStatus::Registered);
    }
}

/// **A stop between the artifact and the arming:** the record is written
/// before the artifact, so it is there unarmed (no row); the send is
/// refused, and the pass's re-send is admitted only once the hook armed
/// the record. An unshield artifact with no record (dispatched before
/// records were kept) is admitted as before.
#[test]
#[ignore = "P13a placement ruling: a refusal after the artifact settles it not_executed and upstream retires the record, so a refused unshield is never re-sent (ported: an_unshield_refused_before_its_arming_is_settled_and_a_stopped_one_is_armed_at_re_send)"]
fn unshield_resend_is_admitted_only_once_its_record_is_armed() {
    let h = Hooked::new();
    let fee = UNSHIELD_FEE;
    let payload = h.staged_unshield(
        1,
        &[1],
        unshield_bytes(1, &[1], own_platform(), NOTE / 2 + fee),
    );
    let hash = payload.st_hash;
    h.intend(own_platform(), NOTE / 2);
    super::super::platform_moves::tests::fail_next_arm_of(&h.f.session);
    let refused = h.dispatch(payload).unwrap_err();
    assert!(refused.contains("not armed"), "{refused}");
    assert!(h.artifact(1).is_some());
    let records = h.records();
    assert_eq!((records.len(), records[0].armed), (1, false));
    assert_eq!(h.intended_record(), None);

    h.lease();
    super::super::platform_moves::tests::fail_next_arm_of(&h.f.session);
    let refused = h.admit(SHIELDED_ACCOUNT, 1, hash).unwrap_err();
    assert!(refused.contains("not armed"), "{refused}");
    assert!(!h.records()[0].armed);
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, hash), Ok(()));
    assert!(h.records()[0].armed, "armed before the re-send");

    let (s, wallet) = (Arc::clone(&h.f.session), h.f.wallet);
    let legacy = dw_appdb::dispatch::NewShieldedArtifact {
        activity: [2; 32],
        origin: dw_appdb::dispatch::ArtifactOrigin::Dispatched,
        st_hash: Some([0x22; 32]),
        kind: Some(ArtifactKind::Unshield),
        anchor: Some([0xa0; 32]),
        identity_id: None,
        selected: vec![[0x92; 32]],
        published: vec![[0x92; 32], [0xee; 32]],
    };
    let wrote = (h.f.engine)
        .block_on(async move { s.leases.record_shielded_artifact(wallet, legacy).await });
    assert_eq!(wrote, Ok(true));
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 2, [0x22; 32]), Ok(()));
    assert_eq!(h.records().len(), 1);
}

/// An unshield signed by dpp's own builder (`build_unshield_transition`,
/// the one `shielded_unshield_to_with_keyset` signs with, and the real
/// Orchard prover) for the engine's quote is admitted by the hook: the
/// builder signs the credits plus the fee the quote priced, which is the
/// quote's total, and the hook arms the record with the quoted credits.
#[test]
fn an_unshield_the_library_builds_for_the_quote_is_admitted() {
    use dpp::address_funds::OrchardAddress;
    use dpp::serialization::PlatformSerializable;
    use dpp::shielded::builder::{SpendableNote, build_unshield_transition};
    use dpp::version::PlatformVersion;
    use grovedb_commitment_tree::{
        ExtractedNoteCommitment, FullViewingKey, Hashable, MerkleHashOrchard, MerklePath,
        NOTE_COMMITMENT_TREE_DEPTH, Note, NoteValue, RandomSeed, Rho, Scope, SpendAuthorizingKey,
        SpendingKey,
    };
    let h = Hooked::new();
    let credits = NOTE / 2;
    let lib = super::FakeShielded::default();
    let quote = (h.f.engine)
        .block_on(async { pool::unshield_to_platform_quote(&lib, credits, None).await })
        .unwrap();
    assert_eq!(quote.total_credits, credits + quote.fee_credits);

    let sk = SpendingKey::from_bytes([42; 32]).unwrap();
    let fvk = FullViewingKey::from(&sk);
    let address = fvk.address_at(0u32, Scope::External);
    let rho: Rho = Option::from(Rho::from_bytes(&[0; 32])).unwrap();
    let rseed: RandomSeed = Option::from(RandomSeed::from_bytes([1; 32], &rho)).unwrap();
    let note: Note = Option::from(Note::from_parts(
        address,
        NoteValue::from_raw(NOTE),
        rho,
        rseed,
    ))
    .unwrap();
    let merkle_path = MerklePath::from_parts(
        0,
        [MerkleHashOrchard::empty_leaf(); NOTE_COMMITMENT_TREE_DEPTH],
    );
    let anchor = merkle_path.root(ExtractedNoteCommitment::from(note.commitment()));
    let spend = SpendableNote { note, merkle_path };
    let change = OrchardAddress::from_raw_bytes(&address.to_raw_address_bytes()).unwrap();
    let prover = platform_wallet::wallet::shielded::CachedOrchardProver;
    let (st, fee) = build_unshield_transition(
        vec![spend],
        own_platform(),
        credits,
        &change,
        &fvk,
        &SpendAuthorizingKey::from(&sk),
        anchor,
        &&prover,
        [0; 36],
        PlatformVersion::get(lib.version).unwrap(),
    )
    .unwrap();
    assert_eq!(fee, quote.fee_credits, "the builder's fee is the quote's");

    h.intend_total(own_platform(), quote.credits, quote.total_credits);
    let payload = h.staged_unshield(1, &[1], st.serialize_to_bytes().unwrap());
    assert_eq!(h.dispatch(payload), Ok(()));
    let records = h.records();
    assert_eq!(
        (records.len(), records[0].credits, records[0].armed),
        (1, credits, true)
    );
}
