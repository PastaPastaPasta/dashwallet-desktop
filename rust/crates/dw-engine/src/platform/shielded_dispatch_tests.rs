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
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::shielded::store::PendingRedrive;
use platform_wallet::wallet::shielded::{
    CachedOrchardProver, DispatchedNote, NetworkShieldedCoordinator, OrchardKeySet,
    ShieldedDispatch, ShieldedDispatchKind, ShieldedDispatchPayload, ShieldedStore, SubwalletId,
};
use sha2::{Digest, Sha256};

use super::super::shielded_pool::{self as pool, EngineDispatch, ShieldedLib, on_pool};
use super::super::topup_tests::{self, PASS, options};
use super::{NOTE, bound_address, core_address, note, payee};
use crate::{DashNetwork, NetworkSession};

/// A nullifier no note of the account has: a padding action's.
const PADDING: [u8; 32] = [0xee; 32];

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
        };
        self.f.engine.block_on(async move {
            (coordinator.store().write().await)
                .arm_unsent_redrive(id, redrive)
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

    /// The wallet's background lease, created while the vault is unlocked.
    fn lease(&self) {
        let (s, wallet) = (Arc::clone(&self.f.session), self.f.wallet);
        self.f.engine.block_on(async move {
            s.ensure_background().await;
            assert!(s.leases.background(&wallet).is_some());
        });
    }

    /// The attempts of the stored spend `n`; `None` once it is gone.
    fn attempts(&self, n: u8) -> Option<u32> {
        let (coordinator, wallet) = (Arc::clone(&self.coordinator), self.f.wallet.0);
        (self.f.engine)
            .block_on(async move { coordinator.pending_spend_redrives(wallet).await })
            .unwrap()
            .into_iter()
            .find(|(_, r)| r.activity_id == [n; 32])
            .map(|(_, r)| r.attempts)
    }
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

async fn recording(coordinator: &NetworkShieldedCoordinator) -> bool {
    (coordinator.store().read().await).records_spend_dispositions()
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
        assert_eq!(h.artifact(n).unwrap().origin, origin);
        let refused = h.admit(SHIELDED_ACCOUNT, n, st_hash(n)).unwrap_err();
        assert!(refused.contains("does not allow"), "{refused}");
    }
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

/// Once the session's close has shut its gate the hook refuses both ways,
/// with the wallet's lease still held: a pass in flight re-sends nothing
/// more (the stored spend keeps its attempts) and no new spend is accepted
/// or written.
#[test]
fn close_during_redrive_pass_admit_refuses_without_attempt_bump() {
    let h = Hooked::new();
    h.lease();
    assert_eq!(h.dispatch(h.staged(1, &[1])), Ok(()));
    let unsent = h.staged(2, &[2]);
    assert_eq!(h.admit(SHIELDED_ACCOUNT, 1, st_hash(1)), Ok(()));

    let (s, hook, wallet) = (Arc::clone(&h.f.session), Arc::clone(&h.hook), h.f.wallet);
    h.f.engine.block_on(async move {
        let _closing = s.gate.close().await;
        assert!(!s.is_open());
        assert!(s.leases.background(&wallet).is_some());
        let refused = (hook
            .admit(wallet.0, SHIELDED_ACCOUNT, [1; 32], st_hash(1))
            .await)
            .unwrap_err()
            .reason;
        assert!(refused.contains("closing"), "{refused}");
        let refused = hook.dispatch(unsent).await.unwrap_err().reason;
        assert!(refused.contains("closing"), "{refused}");
    });
    assert_eq!((h.attempts(1), h.attempts(2)), (Some(0), Some(0)));
    assert_eq!(h.artifact(2), None);
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
    assert!(f.engine.block_on(recording(&coordinator)));
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
    assert!(f.engine.block_on(recording(&coordinator)));

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
        assert!(recording(&coordinator).await);
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
        assert!(!recording(&bare).await);
        assert_eq!(
            pool::admit_pass(&s, &m).await.err(),
            Some(pool::PassHeld::NoHook)
        );
        assert!(!pool::guarded_pass(&s, &m, true).await);

        let opened = pool::coordinator(&s, &m, None).await.unwrap();
        assert!(Arc::ptr_eq(&opened, &bare));
        assert!(recording(&bare).await);
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        // Set once: a later open leaves it.
        pool::coordinator(&s, &m, None).await.unwrap();
        assert!(recording(&bare).await);
    });
}

/// The library refuses a key set that is not the account's before it
/// selects, builds, stores or asks the hook anything, on both spends, and
/// the facade reads that as a refusal with nothing sent.
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
